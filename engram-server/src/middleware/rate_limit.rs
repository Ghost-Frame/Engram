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
    if OPEN_PATHS
        .iter()
        .any(|p| path == *p || path.starts_with(&format!("{}/", p)))
    {
        return next.run(request).await;
    }

    let auth_ctx = request.extensions().get::<AuthContext>().cloned();

    let (user_id, limit) = match auth_ctx {
        Some(ctx) => (ctx.user_id, ctx.key.rate_limit as i64),
        // Unauthenticated requests are handled by auth middleware; pass through here.
        None => return next.run(request).await,
    };

    let key = format!("user:{}", user_id);

    match ratelimit::check_and_increment(&state.db, &key, limit, 60).await {
        Ok(true) => next.run(request).await,
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
            // SECURITY: fail CLOSED on backend errors for authenticated
            // requests. Previously we passed the request through on error,
            // which turned any flaky query into a rate-limit bypass: an
            // attacker could intentionally poison the rate_limits table (e.g.
            // via heavy write contention) to get unlimited throughput.
            tracing::error!("rate_limit check failed for {}: {}", key, e);
            let body = serde_json::json!({
                "error": "Rate limit backend unavailable. Retry shortly.",
                "retry_after": 5,
            });
            axum::response::Response::builder()
                .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                .header("Content-Type", "application/json")
                .header("Retry-After", "5")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap_or_else(|_| axum::response::Response::new(axum::body::Body::empty()))
        }
    }
}
