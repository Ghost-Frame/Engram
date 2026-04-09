use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use engram_lib::auth::AuthContext;

use crate::state::AppState;

#[allow(dead_code)]
/// Axum middleware that logs every HTTP request to the audit trail.
///
/// Runs after auth middleware so that `AuthContext` is available in extensions.
/// Uses fire-and-forget logging so it never delays the response.
pub async fn audit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // Capture pre-request fields before the request is consumed.
    let auth_ctx = request.extensions().get::<AuthContext>().cloned();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let ip = request
        .headers()
        .get("x-forwarded-for")
        .or_else(|| request.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let response = next.run(request).await;
    let status = response.status().as_u16();

    // Spawn fire-and-forget: audit write must not block the response.
    let db = state.db.clone();
    tokio::spawn(async move {
        let (user_id, agent_id) = auth_ctx
            .map(|ctx| (Some(ctx.user_id), ctx.key.agent_id))
            .unwrap_or((None, None));

        let action = format!("http.{}", method.to_lowercase());
        let details = format!("path={} status={}", path, status);

        if let Err(e) = engram_lib::audit::log_request(
            &db,
            user_id,
            agent_id,
            &action,
            Some("http"),
            None,
            Some(&details),
            ip.as_deref(),
            None,
        ).await {
            tracing::warn!("audit log failed: {}", e);
        }
    });

    response
}
