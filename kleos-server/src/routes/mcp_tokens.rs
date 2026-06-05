//! MCP direct-auth token management routes.
//!
//! POST   /mcp-tokens       -- register a minted token (PIV-envelope auth only)
//! GET    /mcp-tokens       -- list tokens for the authenticated user
//! GET    /mcp-tokens/:jti  -- get single token info
//! DELETE /mcp-tokens/:jti  -- revoke a single token
//! DELETE /mcp-tokens       -- revoke all tokens for the authenticated user

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use kleos_lib::mcp_token;
use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

/// Registers the MCP token management routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/mcp-tokens",
            post(register_token).get(list_tokens).delete(revoke_all),
        )
        .route("/mcp-tokens/{jti}", get(get_token).delete(revoke_token))
}

/// Request body for POST /mcp-tokens.
#[derive(Deserialize)]
struct RegisterTokenBody {
    /// The full kleos. token string.
    token: String,
    /// Human-readable name for this token.
    name: String,
    /// Requested scopes (CSV).
    scopes: String,
    /// Requested TTL in seconds.
    ttl_secs: u64,
}

/// Register a minted MCP token. Requires PIV-envelope auth (identity must be present).
async fn register_token(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Json(body): Json<RegisterTokenBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Enforce PIV-envelope auth only: identity must be present.
    let identity = auth_ctx.identity.as_ref().ok_or_else(|| {
        AppError(kleos_lib::EngError::Auth(
            "MCP token registration requires PIV-envelope authentication (not bearer)".into(),
        ))
    })?;

    // Decode and validate the token.
    let decoded = mcp_token::decode(&body.token).map_err(|e| {
        AppError(kleos_lib::EngError::InvalidInput(format!(
            "invalid token: {}",
            e
        )))
    })?;
    let payload = &decoded.payload;

    // The token's `payload.uid` is NOT trusted: the authenticated signing
    // identity is the sole authority for ownership. We stamp the row with
    // `auth_ctx.user_id` (the verified key owner) below, so a keyless minter
    // (the SO_PEERCRED broker, kleos-cli) need not know its server-side user id.

    // Strict scope validation.
    let requested_scopes = mcp_token::parse_scopes_strict(&body.scopes)
        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?;

    // Scope cap: requested must be subset of caller's scopes.
    for scope in &requested_scopes {
        if !auth_ctx.has_scope(scope) {
            return Err(AppError(kleos_lib::EngError::Auth(format!(
                "cannot grant scope '{}' that caller does not hold",
                scope
            ))));
        }
    }

    // TTL cap.
    let max_ttl: u64 = std::env::var("KLEOS_MCP_TOKEN_MAX_TTL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(mcp_token::DEFAULT_MAX_TTL_SECS);
    if body.ttl_secs > max_ttl {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "TTL {}s exceeds maximum {}s",
            body.ttl_secs, max_ttl
        ))));
    }

    // Look up the identity key's pubkey_pem to verify token signature.
    let ik_id = identity.identity_key_id;
    let pubkey_pem = state
        .db
        .read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT pubkey_pem FROM identity_keys WHERE id = ?1 AND is_active = 1",
                    params![ik_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })
        .await?
        .ok_or_else(|| {
            AppError(kleos_lib::EngError::Auth(
                "identity key not found or revoked".into(),
            ))
        })?;

    // Verify the token's signature against the caller's identity key.
    let vk = kleos_lib::auth_piv::pem_to_ed25519_verifying_key(&pubkey_pem).map_err(|e| {
        AppError(kleos_lib::EngError::Auth(format!(
            "invalid identity key: {}",
            e
        )))
    })?;
    mcp_token::verify_signature(&vk, &decoded).map_err(|_| {
        AppError(kleos_lib::EngError::Auth(
            "token signature does not match authenticated identity key".into(),
        ))
    })?;

    // Insert into mcp_tokens table. Owner is the authenticated key owner, not
    // the client-supplied payload.uid (which is ignored).
    let jti = payload.jti.clone();
    let uid = auth_ctx.user_id;
    let tid = payload.tid;
    let kid = payload.kid.clone();
    let scopes_str = body.scopes.clone();
    let name = body.name.clone();
    let exp_str = chrono::DateTime::from_timestamp(payload.exp as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();
    let exp_str_ret = exp_str.clone();

    state
        .db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO mcp_tokens (jti, user_id, tenant_id, identity_key_id, kid, name, scopes, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![jti, uid, tid, ik_id, kid, name, scopes_str, exp_str],
            )
            .map_err(|e| {
                if let rusqlite::Error::SqliteFailure(ref err, _) = e {
                    if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE {
                        return kleos_lib::EngError::Conflict(
                            "jti already registered (collision)".into(),
                        );
                    }
                }
                kleos_lib::EngError::Database(e)
            })?;
            Ok(())
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "registered": true,
            "jti": payload.jti,
            "name": body.name,
            "scopes": body.scopes,
            "expires_at": exp_str_ret,
        })),
    ))
}

/// List all MCP tokens for the authenticated user.
async fn list_tokens(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    let uid = auth_ctx.user_id;
    let tokens = state
        .db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT jti, name, scopes, is_active, issued_at, expires_at, last_used_at
                     FROM mcp_tokens WHERE user_id = ?1
                     ORDER BY created_at DESC",
            )?;
            let rows = stmt
                .query_map(params![uid], |row| {
                    Ok(json!({
                        "jti": row.get::<_, String>(0)?,
                        "name": row.get::<_, String>(1)?,
                        "scopes": row.get::<_, String>(2)?,
                        "is_active": row.get::<_, bool>(3)?,
                        "issued_at": row.get::<_, String>(4)?,
                        "expires_at": row.get::<_, String>(5)?,
                        "last_used_at": row.get::<_, Option<String>>(6)?,
                    }))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "tokens": tokens, "count": tokens.len() })))
}

/// Get info for a single MCP token (user-scoped).
async fn get_token(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Path(jti): Path<String>,
) -> Result<Json<Value>, AppError> {
    let uid = auth_ctx.user_id;
    let token = state
        .db
        .read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT jti, name, scopes, is_active, issued_at, expires_at,
                        last_used_at, kid, revoked_at, revoke_reason
                 FROM mcp_tokens WHERE jti = ?1 AND user_id = ?2",
                    params![jti, uid],
                    |row| {
                        Ok(json!({
                            "jti": row.get::<_, String>(0)?,
                            "name": row.get::<_, String>(1)?,
                            "scopes": row.get::<_, String>(2)?,
                            "is_active": row.get::<_, bool>(3)?,
                            "issued_at": row.get::<_, String>(4)?,
                            "expires_at": row.get::<_, String>(5)?,
                            "last_used_at": row.get::<_, Option<String>>(6)?,
                            "kid": row.get::<_, String>(7)?,
                            "revoked_at": row.get::<_, Option<String>>(8)?,
                            "revoke_reason": row.get::<_, Option<String>>(9)?,
                        }))
                    },
                )
                .optional()?)
        })
        .await?;

    match token {
        Some(t) => Ok(Json(t)),
        None => Err(AppError(kleos_lib::EngError::NotFound(
            "token not found".into(),
        ))),
    }
}

/// Revoke a single MCP token (user-scoped by jti + user_id).
async fn revoke_token(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
    Path(jti): Path<String>,
) -> Result<Json<Value>, AppError> {
    let uid = auth_ctx.user_id;
    let jti_ret = jti.clone();
    let affected = state
        .db
        .write(move |conn| {
            let n = conn.execute(
                "UPDATE mcp_tokens SET is_active = 0, revoked_at = datetime('now')
                     WHERE jti = ?1 AND user_id = ?2 AND is_active = 1",
                params![jti, uid],
            )?;
            Ok(n)
        })
        .await?;

    if affected == 0 {
        return Err(AppError(kleos_lib::EngError::NotFound(
            "token not found or already revoked".into(),
        )));
    }

    Ok(Json(json!({ "revoked": true, "jti": jti_ret })))
}

/// Revoke all MCP tokens for the authenticated user.
async fn revoke_all(
    State(state): State<AppState>,
    Auth(auth_ctx): Auth,
) -> Result<Json<Value>, AppError> {
    let uid = auth_ctx.user_id;
    let affected = state
        .db
        .write(move |conn| {
            let n = conn.execute(
                "UPDATE mcp_tokens SET is_active = 0, revoked_at = datetime('now')
                     WHERE user_id = ?1 AND is_active = 1",
                params![uid],
            )?;
            Ok(n)
        })
        .await?;

    Ok(Json(json!({ "revoked_count": affected })))
}
