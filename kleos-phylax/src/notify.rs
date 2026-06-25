//! Best-effort out-of-band approval notification.
//!
//! When an M3 approval is raised, phylaxd can POST a summary of it -- plus a
//! single-use capability token -- to an external notifier so a human can be
//! prompted to decide out of band. This is disabled (a no-op) unless
//! `PHYLAX_NOTIFY_URL` is set, carries no transport-specific logic, and never
//! fails the calling request: a notifier that is down or slow must not block or
//! break an approval, which can still be decided through the authenticated
//! `PUT /phylax/approvals/{id}` path.

use serde_json::json;

/// Fire-and-forget POST of an approval to the configured notifier. No-op unless
/// `PHYLAX_NOTIFY_URL` is set. Never returns an error to the caller.
#[allow(clippy::too_many_arguments)]
pub async fn notify_approval(
    approval_id: i64,
    action: &str,
    agent_name: &str,
    category: &str,
    secret_name: &str,
    expires_at: &str,
    raw_token: &str,
) {
    let Ok(url) = std::env::var("PHYLAX_NOTIFY_URL") else {
        return;
    };
    let body = json!({
        "approval_id": approval_id,
        "action": action,
        "agent_name": agent_name,
        "category": category,
        "secret_name": secret_name,
        "expires_at": expires_at,
        "decide_token": raw_token,
    });
    let client = reqwest::Client::new();
    if let Err(e) = client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        tracing::warn!(error = %e, "approval notify failed (non-fatal)");
    }
}
