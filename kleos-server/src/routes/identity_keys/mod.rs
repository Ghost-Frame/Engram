//! Identity key management endpoints.
//!
//! Handles enrollment, listing, revocation, and invite generation for
//! cryptographic signing keys (PIV YubiKey, software Ed25519, and
//! eventually FIDO2 security keys).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use rusqlite::params;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::auth::Scope;
use kleos_lib::auth_piv;

mod types;
use types::{CreateInviteBody, EnrollBody, ListParams, RevokeBody};

/// Registers all identity key management routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/identity-keys/enroll", post(enroll_handler))
        .route("/identity-keys", get(list_handler))
        .route("/identity-keys/mine", get(list_mine_handler))
        .route("/identity-keys/{id}/revoke", post(revoke_handler))
        .route("/identity-keys/invite", post(create_invite_handler))
}

/// Enrolls a new signing key for the authenticated user after verifying
/// a proof-of-possession signature over the key material.
async fn enroll_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<EnrollBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let algo = auth_piv::SignatureAlgo::from_header(&body.algo).map_err(AppError)?;

    if !["piv", "soft"].contains(&body.tier.as_str()) {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "tier must be 'piv' or 'soft'".into(),
        )));
    }

    let proof_msg = format!(
        "KLEOS-ENROLL:{}:{}:{}:{}",
        body.algo, body.tier, body.host_label, body.pubkey_pem
    );
    auth_piv::verify_signature(algo, &body.pubkey_pem, proof_msg.as_bytes(), &body.sig_hex)?;

    let pubkey_der = pem_to_der(&body.pubkey_pem)?;
    let fingerprint = hex::encode(Sha256::digest(&pubkey_der));

    let user_id = auth.user_id;
    // SECURITY (C3): the new key inherits the caller's own scopes, so a caller
    // cannot mint a key more privileged than itself (mirrors the api-key
    // SEC-MED-4 escalation cap). Stored comma-separated to match the
    // api_keys.scopes / v53 identity_keys.scopes column format.
    let scopes_csv = kleos_lib::auth::scopes_to_string(&auth.key.scopes);
    let tier = body.tier.clone();
    let algo_str = algo.as_str().to_string();
    let pubkey_pem = body.pubkey_pem.clone();
    let fpr = fingerprint.clone();
    let host = body.host_label.clone();
    let label = body.label.clone();
    let serial = body.serial.clone();

    let id: i64 = state.db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO identity_keys (user_id, tier, algo, pubkey_pem, pubkey_fingerprint, host_label, label, serial, scopes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![user_id, tier, algo_str, pubkey_pem, fpr, host, label, serial, scopes_csv],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(conn.last_insert_rowid())
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "pubkey_fingerprint": fingerprint,
            "host_label": body.host_label,
            "tier": body.tier,
        })),
    ))
}

/// Lists all identity keys across all users. Admin-only. Optionally
/// filters to active-only keys (the default).
async fn list_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required for listing all identity keys".into(),
        )));
    }

    let active_only = params.active_only.unwrap_or(true);

    let keys = state.db
        .read(move |conn| {
            let mut stmt = if active_only {
                conn.prepare(
                    "SELECT id, user_id, tier, algo, pubkey_fingerprint, host_label, label, serial, enrolled_at, last_seen_at, is_active
                     FROM identity_keys WHERE is_active = 1 ORDER BY enrolled_at DESC",
                )
            } else {
                conn.prepare(
                    "SELECT id, user_id, tier, algo, pubkey_fingerprint, host_label, label, serial, enrolled_at, last_seen_at, is_active
                     FROM identity_keys ORDER BY enrolled_at DESC",
                )
            }
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map([], |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "user_id": row.get::<_, i64>(1)?,
                        "tier": row.get::<_, String>(2)?,
                        "algo": row.get::<_, String>(3)?,
                        "pubkey_fingerprint": row.get::<_, String>(4)?,
                        "host_label": row.get::<_, String>(5)?,
                        "label": row.get::<_, Option<String>>(6)?,
                        "serial": row.get::<_, Option<String>>(7)?,
                        "enrolled_at": row.get::<_, String>(8)?,
                        "last_seen_at": row.get::<_, Option<String>>(9)?,
                        "is_active": row.get::<_, bool>(10)?,
                    }))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "keys": keys, "count": keys.len() })))
}

/// Lists identity keys belonging to the currently authenticated user.
async fn list_mine_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;

    let keys = state.db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, tier, algo, pubkey_fingerprint, host_label, label, serial, enrolled_at, last_seen_at, is_active
                     FROM identity_keys WHERE user_id = ?1 ORDER BY enrolled_at DESC",
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let rows = stmt
                .query_map(params![user_id], |row| {
                    Ok(json!({
                        "id": row.get::<_, i64>(0)?,
                        "tier": row.get::<_, String>(1)?,
                        "algo": row.get::<_, String>(2)?,
                        "pubkey_fingerprint": row.get::<_, String>(3)?,
                        "host_label": row.get::<_, String>(4)?,
                        "label": row.get::<_, Option<String>>(5)?,
                        "serial": row.get::<_, Option<String>>(6)?,
                        "enrolled_at": row.get::<_, String>(7)?,
                        "last_seen_at": row.get::<_, Option<String>>(8)?,
                        "is_active": row.get::<_, bool>(9)?,
                    }))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(rows)
        })
        .await?;

    Ok(Json(json!({ "keys": keys, "count": keys.len() })))
}

/// Revokes an identity key by ID. Admins can revoke any key; regular
/// users can only revoke their own.
async fn revoke_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path(key_id): Path<i64>,
    Json(body): Json<RevokeBody>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.user_id;
    let is_admin = auth.has_scope(&Scope::Admin);
    let reason = body.reason;

    let revoked = state.db
        .write(move |conn| {
            let affected = if is_admin {
                conn.execute(
                    "UPDATE identity_keys SET is_active = 0, revoked_at = datetime('now'), revoke_reason = ?2
                     WHERE id = ?1 AND is_active = 1",
                    params![key_id, reason],
                )
            } else {
                conn.execute(
                    "UPDATE identity_keys SET is_active = 0, revoked_at = datetime('now'), revoke_reason = ?3
                     WHERE id = ?1 AND is_active = 1 AND user_id = ?2",
                    params![key_id, user_id, reason],
                )
            }
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok(affected > 0)
        })
        .await?;

    if revoked {
        Ok(Json(json!({ "revoked": true, "id": key_id })))
    } else {
        Err(AppError(kleos_lib::EngError::NotFound(
            "identity key not found or already revoked".into(),
        )))
    }
}

/// Generates a one-time enrollment invite token for the given user.
/// The raw token is returned to the caller exactly once -- only the
/// SHA-256 hash is persisted. Tokens expire 24 hours after creation.
async fn create_invite_handler(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<CreateInviteBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(kleos_lib::EngError::Auth(
            "admin scope required to create enrollment invites".into(),
        )));
    }

    // Generate 32 bytes of cryptographic randomness, then URL-safe
    // base64-encode them so the token is safe to paste into a CLI.
    use rand::Rng;
    let mut raw_bytes = [0u8; 32];
    rand::rng().fill(&mut raw_bytes);
    use base64::Engine;
    let raw_token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw_bytes);

    // Store only the hash -- if the DB leaks, the tokens are useless.
    let token_hash = hex::encode(Sha256::digest(raw_token.as_bytes()));

    let user_id = body.user_id;
    let method = body.method.clone();
    let hash_clone = token_hash.clone();
    let method_clone = method.clone();

    // Verify the target user exists and is active before creating an invite.
    let (invite_id, expires_at) = state
        .db
        .write(move |conn| {
            let user_active: bool = conn
                .query_row(
                    "SELECT is_active FROM users WHERE id = ?1",
                    params![user_id],
                    |row| row.get(0),
                )
                .map_err(|_| {
                    kleos_lib::EngError::NotFound(format!("user_id {} not found", user_id))
                })?;

            if !user_active {
                return Err(kleos_lib::EngError::InvalidInput(
                    "target user is deactivated".into(),
                ));
            }

            // 24-hour expiry window gives the admin time to hand the
            // token to the coworker without being too permissive.
            conn.execute(
                "INSERT INTO enrollment_invites (user_id, token_hash, method, expires_at)
                 VALUES (?1, ?2, ?3, datetime('now', 'utc', '+24 hours'))",
                params![user_id, hash_clone, method_clone],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let id = conn.last_insert_rowid();

            // Read back the server-computed expires_at timestamp.
            let exp: String = conn
                .query_row(
                    "SELECT expires_at FROM enrollment_invites WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            Ok((id, exp))
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": invite_id,
            "token": raw_token,
            "user_id": user_id,
            "method": method,
            "expires_at": expires_at,
        })),
    ))
}

/// Extracts the raw DER bytes from a PEM-encoded public key string.
fn pem_to_der(pem: &str) -> Result<Vec<u8>, AppError> {
    let begin = "-----BEGIN PUBLIC KEY-----";
    let end = "-----END PUBLIC KEY-----";
    let b64: String = pem
        .lines()
        .skip_while(|l| !l.starts_with(begin))
        .skip(1)
        .take_while(|l| !l.starts_with(end))
        .collect::<Vec<_>>()
        .join("");
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(format!("bad PEM: {e}"))))
}
