use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use engram_lib::auth::AuthContext;
use engram_lib::ratelimit;

use crate::state::AppState;

const OPEN_PATHS: &[&str] = &["/health", "/live", "/ready", "/bootstrap"];

/// Axum middleware implementing per-user sliding-window rate limiting.
///
/// Uses the DB-backed rate limiter from engram-lib. The limit (requests/minute)
/// is read from the authenticated API key's `rate_limit` field.
///
/// Returns HTTP 429 with a `Retry-After` header when the limit is exceeded.
/// Open paths and unauthenticated requests bypass the limiter.
pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // Skip rate limiting for health/bootstrap paths.
    if OPEN_PATHS.iter().any(|p| path == *p || path.starts_with(&format!("{}/", p))) {
        return next.run(request).await;
    }

    let auth_ctx = request.extensions().get::<AuthContext>().cloned();

    let (user_id, limit) = match auth_ctx {
        Some(ctx) => (ctx.user_id, ctx.key.rate_limit as i64),
        // Unauthenticated requests are handled by auth middleware; pass through here.
        None => return next.run(request).await,
    };

    let key = format!("user:{}", user_id);

    match ratelimit::check_rate_limit(&state.db, &key, limit, 60).await {
        Ok(true) => {
            // Within limit -- record the hit then continue.
            if let Err(e) = ratelimit::increment_counter(&state.db, &key).await {
                tracing::warn!("rate_limit increment failed for {}: {}", key, e);
            }
            next.run(request).await
        }
        Ok(false) => {
            let body = serde_json::json!({
                "error": "Rate limit exceeded.",
                "retry_after": 60,
            });
            axum::response::Response::builder()
                .status(axum::http::StatusCode::TOO_MANY_REQUESTS)
                .header("Content-Type", "application/json")
                .header("Retry-After", "60")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
        }
        Err(e) => {
            // Fail open: log the error and allow the request through.
            tracing::warn!("rate_limit check failed for {}: {}", key, e);
            next.run(request).await
        }
    }
}
