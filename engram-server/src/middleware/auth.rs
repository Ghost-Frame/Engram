use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use engram_lib::auth::{validate_key, ApiKey, AuthContext, Scope};

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
    AuthContext {
        key: ApiKey {
            id: 0,
            user_id: 1,
            key_prefix: "open".into(),
            name: "open-access".into(),
            scopes: vec![Scope::Read, Scope::Write],
            rate_limit: 1000,
            is_active: true,
            agent_id: None,
            last_used_at: None,
            expires_at: None,
            created_at: String::new(),
        },
        user_id: 1,
    }
}

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

    // Check ENGRAM_OPEN_ACCESS env var
    if std::env::var("ENGRAM_OPEN_ACCESS").as_deref() == Ok("1") {
        tracing::warn!(
            path = %path,
            "ENGRAM_OPEN_ACCESS bypassing authentication for request"
        );
        request.extensions_mut().insert(open_access_context());
        return next.run(request).await;
    }

    // Extract Bearer token
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    let method = request.method().clone();
    match token {
        None => {
            let body = serde_json::json!({ "error": "Authentication required. Provide Bearer engram_* token." });
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
                request.extensions_mut().insert(auth_ctx);
                next.run(request).await
            }
            Err(e) => {
                let body = serde_json::json!({ "error": e.to_string() });
                axum::response::Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header("Content-Type", "application/json")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
            }
        },
    }
}
