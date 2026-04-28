use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use kleos_lib::auth::AuthContext;

use crate::middleware::client_ip::client_ip;
use crate::state::AppState;

/// Axum middleware that logs every HTTP request to the audit trail.
///
/// Runs after auth middleware so that `AuthContext` is available in extensions.
/// Uses fire-and-forget logging so it never delays the response.
#[tracing::instrument(skip_all, fields(middleware = "server.audit"))]
pub async fn audit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // Record activity timestamp so the dreamer can gate heavy work behind
    // idleness.  We use a monotonic millisecond counter rather than
    // wall-clock seconds: NTP steps and DST transitions can move
    // SystemTime backwards, which would make the dreamer either spin
    // (elapsed < 0 wraps via saturating_sub) or skip cycles forever.
    state.last_request_time.store(
        crate::dreamer::monotonic_millis(),
        std::sync::atomic::Ordering::Relaxed,
    );

    // Capture pre-request fields before the request is consumed.
    let auth_ctx = request.extensions().get::<AuthContext>().cloned();
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    // SECURITY: share the rate-limiter's trusted-proxy resolver so we
    // don't log attacker-controlled XFF/X-Real-IP values as if they were
    // the caller's real address.
    let ip = client_ip(&request, &state.config.trusted_proxies);

    let response = next.run(request).await;
    let status = response.status().as_u16();

    // Spawn fire-and-forget: audit write must not block the response.
    // Bounded by audit_log_sem (H-005); shutdown-propagated via shutdown_token (M-008).
    let db = state.db.clone();
    let permit = match state.audit_log_sem.clone().acquire_owned().await {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!("audit_log semaphore closed; skipping audit write");
            return response;
        }
    };
    let shutdown = state.shutdown_token.clone();
    let mut bg = state.background_tasks.lock().await;
    bg.spawn(async move {
        let _permit = permit;
        let (user_id, agent_id, identity_id, tier) = auth_ctx
            .map(|ctx| {
                let (iid, t) = ctx
                    .identity
                    .as_ref()
                    .map(|id| (id.identity_id, Some(id.tier.as_str().to_string())))
                    .unwrap_or((None, None));
                (Some(ctx.user_id), ctx.key.agent_id, iid, t)
            })
            .unwrap_or((None, None, None, None));

        let action = format!("http.{}", method.to_lowercase());
        let details = format!("path={} status={}", path, status);

        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::debug!("background audit_log drained on shutdown");
            }
            _ = async {
                if let Err(e) = kleos_lib::audit::log_request(
                    &db,
                    user_id,
                    agent_id,
                    &action,
                    Some("http"),
                    None,
                    Some(&details),
                    ip.as_deref(),
                    None,
                    identity_id,
                    tier.as_deref(),
                )
                .await
                {
                    tracing::warn!("audit log failed: {}", e);
                }
            } => {}
        }
    });

    response
}
