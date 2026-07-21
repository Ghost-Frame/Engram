use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use kleos_lib::auth::{self, Scope};
use kleos_lib::quota;
use kleos_lib::ratelimit;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

mod types;
use types::{CreateApiKeyBody, RecordUsageBody};

/// Build the security route family (API keys, rate-limit status, quota).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api-keys",
            post(create_api_key_handler).get(list_api_keys_handler),
        )
        .route(
            "/api-keys/{id}",
            axum::routing::delete(delete_api_key_handler),
        )
        .route("/rate-limit/{key}", get(rate_limit_status_handler))
        .route("/quota", get(get_quota_handler))
        .route("/usage", post(record_usage_handler))
}

/// POST /security/api-keys -- mint a key with validated, deduped scopes;
/// '*' maps to admin ([95]) and non-admins cannot grant scopes they lack.
async fn create_api_key_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateApiKeyBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // SECURITY: a caller cannot mint a key with broader scopes than their own.
    // Admin callers may mint anything; non-admins are capped at read/write.
    let requested_raw = body.scopes.as_deref().unwrap_or("read").trim();
    let requested: Vec<String> = requested_raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if requested.is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "scopes must be one of: read, write, admin".into(),
        )));
    }
    let wants_admin = requested.iter().any(|s| s == "admin" || s == "*");
    if wants_admin && !auth.has_scope(&Scope::Admin) {
        return Err(AppError::from(kleos_lib::EngError::Auth(
            "admin scope required to mint admin keys".into(),
        )));
    }
    for s in &requested {
        match s.as_str() {
            "read" | "write" | "admin" | "*" => {}
            other => {
                return Err(AppError::from(kleos_lib::EngError::InvalidInput(format!(
                    "unknown scope: {}",
                    other
                ))));
            }
        }
    }
    // Non-admin callers also cannot elevate to scopes they themselves lack.
    if !auth.has_scope(&Scope::Admin) {
        for s in &requested {
            let scope = match s.as_str() {
                "read" => Scope::Read,
                "write" => Scope::Write,
                _ => continue,
            };
            if !auth.has_scope(&scope) {
                return Err(AppError::from(kleos_lib::EngError::Auth(format!(
                    "caller lacks {} scope and cannot grant it",
                    s
                ))));
            }
        }
    }
    // Rate limit cannot exceed caller's own limit (admins still capped at a sane ceiling).
    let caller_limit = auth.key.rate_limit as i64;
    let max_limit = if auth.has_scope(&Scope::Admin) {
        body.rate_limit.unwrap_or(caller_limit).min(100_000)
    } else {
        body.rate_limit.unwrap_or(caller_limit).min(caller_limit)
    };
    let rate_limit = max_limit.max(1);
    let key_name = body.name.as_deref().unwrap_or("api-key").trim().to_string();
    // Finding [95]: '*' passes the accepted-token check above but does not
    // parse as a Scope, so filter_map silently dropped it and a caller who
    // requested '*' (and held admin) received a key with NO scopes. Map it to
    // admin explicitly and dedupe so '*,read' does not double-insert.
    let mut scopes_vec: Vec<Scope> = Vec::with_capacity(requested.len());
    for s in &requested {
        let scope = if s == "*" {
            Scope::Admin
        } else {
            match s.parse::<Scope>() {
                Ok(sc) => sc,
                // Unreachable: the token allowlist above already rejected
                // anything else. Kept as a hard error rather than a silent
                // skip so a future allowlist/parser drift fails loudly.
                Err(_) => {
                    return Err(AppError::from(kleos_lib::EngError::InvalidInput(format!(
                        "unknown scope: {}",
                        s
                    ))));
                }
            }
        };
        if !scopes_vec.contains(&scope) {
            scopes_vec.push(scope);
        }
    }
    let (key_record, full_key) = auth::create_key(
        &state.db,
        auth.user_id,
        &key_name,
        scopes_vec,
        Some(rate_limit),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "key": key_record, "full_key": full_key })),
    ))
}

/// GET /security/api-keys -- list the caller's keys (admin sees all).
async fn list_api_keys_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let keys = auth::list_keys(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "keys": keys })))
}

/// DELETE /security/api-keys/{id} -- revoke a key.
async fn delete_api_key_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // Admins can revoke any key; regular users can only revoke their own.
    if auth.has_scope(&Scope::Admin) {
        auth::revoke_key_admin(&state.db, id).await?;
    } else {
        auth::revoke_key(&state.db, auth.user_id, id).await?;
    }
    Ok(Json(json!({ "deleted": true, "id": id })))
}

/// GET /security/rate-limit -- current window usage for the caller's key.
async fn rate_limit_status_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    // Admins can check any key; regular users can only check their own
    let check_key = if auth.has_scope(&Scope::Admin) {
        key
    } else {
        // Force the key to be the caller's own rate limit key
        format!("user:{}", auth.user_id)
    };
    let limit = auth.key.rate_limit as i64;
    let allowed = ratelimit::check_rate_limit(&state.db, &check_key, limit, 60).await?;
    Ok(Json(
        json!({ "key": check_key, "allowed": allowed, "limit": limit }),
    ))
}

/// GET /security/quota -- storage quota and usage for the caller.
async fn get_quota_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let status = quota::check_quota(&state.db, auth.user_id).await?;
    Ok(Json(json!(status)))
}

/// POST /security/usage -- record a usage sample against the caller's quota.
async fn record_usage_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RecordUsageBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Clamp quantity to a sane non-negative range.
    let quantity = body.quantity.unwrap_or(1).clamp(0, 10_000);
    // Validate agent_id belongs to the authenticated user (if provided).
    if let Some(agent_id) = body.agent_id {
        let owned = state
            .db
            .read(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM agents WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![agent_id, auth.user_id],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .await?;
        if owned == 0 {
            return Err(AppError(kleos_lib::EngError::Forbidden(
                "agent_id does not belong to the authenticated user".to_string(),
            )));
        }
    }
    quota::record_usage(
        &state.db,
        auth.user_id,
        body.agent_id,
        &body.event_type,
        quantity,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "recorded": true }))))
}
