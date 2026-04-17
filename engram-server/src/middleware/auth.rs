use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use engram_lib::auth::{validate_key, ApiKey, AuthContext, Scope};
use tracing::Instrument;

use crate::state::AppState;

const OPEN_PATHS: &[&str] = &["/health", "/live", "/ready", "/bootstrap"];

/// Methods that mutate state and therefore require `Scope::Write` (or admin).
fn requires_write_scope(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

fn forbid(msg: &str) -> Response {
    let body = serde_json::json!({ "error": msg });
    axum::response::Response::builder()
        .status(StatusCode::FORBIDDEN)
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
}

fn open_access_context() -> AuthContext {
    // SECURITY (SEC-CRIT-2 / SEC-HIGH-4 / MT-F24): open-access grants only
    // Scope::Read. Write scope was removed because any caller with the env
    // var set could otherwise mint arbitrary memories for user 1 with no
    // audit trail. Writes now require a real API key even in open-access
    // mode.
    AuthContext {
        key: ApiKey {
            id: 0,
            user_id: 1,
            key_prefix: "open".into(),
            name: "open-access".into(),
            scopes: vec![Scope::Read],
            rate_limit: 1000,
            is_active: true,
            agent_id: None,
            last_used_at: None,
            expires_at: None,
            created_at: String::new(),
            hash_version: 1,
        },
        user_id: 1,
    }
}

/// SECURITY (SEC-CRIT-2 / MT-F24): in release builds, refuse to honor
/// `ENGRAM_OPEN_ACCESS=1` unless the operator has also set the explicit
/// escape hatch `ENGRAM_ALLOW_OPEN_ACCESS_IN_RELEASE=yes-i-am-sure`. This
/// prevents the dev convenience env var from accidentally being set in a
/// production deployment and silently disabling all auth.
fn open_access_allowed() -> bool {
    if std::env::var("ENGRAM_OPEN_ACCESS").as_deref() != Ok("1") {
        return false;
    }
    if cfg!(debug_assertions) {
        return true;
    }
    matches!(
        std::env::var("ENGRAM_ALLOW_OPEN_ACCESS_IN_RELEASE").as_deref(),
        Ok("yes-i-am-sure")
    )
}

#[tracing::instrument(skip_all, fields(middleware = "server.auth"))]
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Skip auth for public paths
    if OPEN_PATHS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{}/", p)))
    {
        return next.run(request).await;
    }

    // Extract Bearer token first -- a valid API key always takes precedence
    // over open-access mode.
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let method = request.method().clone();

    // If a token is provided, validate it regardless of open-access mode.
    // Open-access only applies when NO token is provided.
    match token {
        None => {
            // No token -- check if open-access mode allows this request
            if open_access_allowed() {
                // Open-access grants read-only; writes still need a real key
                if requires_write_scope(&method) {
                    return forbid("ENGRAM_OPEN_ACCESS is read-only; writes require an API key");
                }
                tracing::warn!(
                    path = %path,
                    "ENGRAM_OPEN_ACCESS bypassing authentication for request"
                );
                request.extensions_mut().insert(open_access_context());
                return next.run(request).await;
            }
            // No token and no open-access -- reject
            let body = serde_json::json!({ "error": "Authentication required. Provide Bearer eg_* token." });
            axum::response::Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("Content-Type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
        }
        Some(raw_key) => match validate_key(&state.db, &raw_key).await {
            Ok(auth_ctx) => {
                // SECURITY: enforce scope by HTTP method. Read-only keys must
                // not be able to POST/PUT/PATCH/DELETE. Admin scope implies
                // write via `has_scope`. Reads always pass.
                if requires_write_scope(&method) && !auth_ctx.has_scope(&Scope::Write) {
                    return forbid("write scope required for this method");
                }
                if !requires_write_scope(&method) && !auth_ctx.has_scope(&Scope::Read) {
                    return forbid("read scope required for this method");
                }
                let user_id = auth_ctx.user_id;
                request.extensions_mut().insert(auth_ctx);
                // MT-F25: attach per-request span so all downstream logs carry user_id.
                let span = tracing::info_span!(
                    "request",
                    user_id = user_id,
                    method = %method,
                    path = %path,
                );
                next.run(request).instrument(span).await
            }
            Err(e) => {
                // SECURITY: normalize the client-facing error so an attacker
                // cannot distinguish "key not found" from "key expired" from
                // "key revoked" and build a probing oracle. The detailed
                // reason is still written to logs for operators.
                tracing::warn!(
                    error = %e,
                    "authentication failed"
                );
                let body = serde_json::json!({ "error": "invalid credentials" });
                axum::response::Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
            }
        },
    }
}
