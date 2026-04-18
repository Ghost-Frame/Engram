use axum::extract::State;
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::sync::atomic::Ordering;

use crate::state::AppState;

/// Rejects mutating HTTP methods with 503 when safe mode is active.
/// GET and HEAD pass through so operators can still inspect state.
#[tracing::instrument(skip_all, fields(middleware = "server.safe_mode"))]
pub async fn safe_mode_middleware(
    State(state): State<AppState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if state.safe_mode.load(Ordering::Relaxed) {
        let method = request.method();
        if matches!(
            method,
            &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
        ) {
            // Allow the safe-mode exit route so operators can recover.
            if request.uri().path() == "/admin/safe-mode/exit" {
                return next.run(request).await;
            }
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({
                    "error": "server is in safe mode due to crash loop",
                    "hint": "POST /admin/safe-mode/exit to recover"
                })),
            )
                .into_response();
        }
    }
    next.run(request).await
}
