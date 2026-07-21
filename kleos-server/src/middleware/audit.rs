use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use kleos_lib::auth::AuthContext;
use kleos_lib::db::Database;
use tokio_util::sync::CancellationToken;

use crate::middleware::client_ip::client_ip;
use crate::state::AppState;

/// Capacity of the audit event channel. Sized so short DB stalls do not drop
/// events, while a sustained stall sheds load instead of queueing unboundedly.
const AUDIT_CHANNEL_CAPACITY: usize = 1024;

/// One HTTP request's audit fields, captured in the middleware and written to
/// the audit trail by the dedicated worker task.
pub struct AuditEvent {
    /// Authenticated user id, when the request carried auth.
    pub user_id: Option<i64>,
    /// Agent id from the API key, when present.
    pub agent_id: Option<i64>,
    /// Identity-key id, when the request was PIV/identity authenticated.
    pub identity_id: Option<i64>,
    /// Identity tier label (e.g. "piv", "soft"), when present.
    pub tier: Option<String>,
    /// HTTP method (lowercased into the action string by the worker).
    pub method: String,
    /// Request path.
    pub path: String,
    /// Response status code.
    pub status: u16,
    /// Trusted-proxy-resolved client IP, if determinable.
    pub ip: Option<String>,
}

/// Spawn the dedicated audit-log worker and return the sender half.
///
/// Finding [57]: the middleware previously awaited a semaphore permit and the
/// background JoinSet mutex on the response path, so audit backpressure (or
/// lock contention) delayed every response. The worker owns the only DB writes;
/// the middleware just try_sends into the bounded channel. On shutdown the
/// worker drains what is already queued, then exits.
pub fn spawn_audit_worker(
    db: Arc<Database>,
    shutdown: CancellationToken,
) -> tokio::sync::mpsc::Sender<AuditEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AuditEvent>(AUDIT_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                _ = shutdown.cancelled() => {
                    // Drain without waiting for new events, then stop.
                    while let Ok(ev) = rx.try_recv() {
                        write_audit_event(&db, ev).await;
                    }
                    tracing::debug!("audit-log worker drained on shutdown");
                    break;
                }
                ev = rx.recv() => match ev {
                    Some(ev) => ev,
                    // All senders dropped (server tearing down).
                    None => break,
                },
            };
            write_audit_event(&db, event).await;
        }
    });
    tx
}

/// Write a single audit event to the audit trail; failures are logged, never
/// propagated (audit is best-effort by design).
async fn write_audit_event(db: &Database, ev: AuditEvent) {
    let action = format!("http.{}", ev.method.to_lowercase());
    let details = format!("path={} status={}", ev.path, ev.status);
    if let Err(e) = kleos_lib::audit::log_request(
        db,
        ev.user_id,
        ev.agent_id,
        &action,
        Some("http"),
        None,
        Some(&details),
        ev.ip.as_deref(),
        None,
        ev.identity_id,
        ev.tier.as_deref(),
    )
    .await
    {
        tracing::warn!("audit log failed: {}", e);
    }
}

/// Axum middleware that logs every HTTP request to the audit trail.
///
/// Runs after auth middleware so that `AuthContext` is available in extensions.
/// The response path performs no awaits after the handler returns: the event is
/// try_sent into the bounded worker channel and dropped (with a warning) when
/// the channel is full or closed (finding [57]).
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

    if let Err(e) = state.audit_tx.try_send(AuditEvent {
        user_id,
        agent_id,
        identity_id,
        tier,
        method,
        path,
        status,
        ip,
    }) {
        // Channel full (sustained DB stall) or worker gone: shed the event
        // rather than blocking the response. try_send makes this observable
        // where the old semaphore starvation was silent queueing.
        tracing::warn!("audit event dropped: {}", e);
    }

    response
}
