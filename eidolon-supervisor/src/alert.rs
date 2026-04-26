use crate::checks::Violation;
use crate::watch::SupervisorState;
use serde_json::json;

pub async fn send_alert(state: &SupervisorState, violation: &Violation) {
    send_inbox(state, violation).await;
    send_axon(state, violation).await;
}

async fn send_inbox(state: &SupervisorState, violation: &Violation) {
    let url = format!("{}/inbox", state.kleos_url);

    let body = json!({
        "content": format!("[supervisor:{}] {}", violation.rule_id, violation.message),
        "category": "alert",
        "importance": severity_to_importance(&violation.severity),
        "tags": ["eidolon-supervisor", &violation.rule_id],
        "source": "eidolon-supervisor",
    });

    let mut req = state.client.post(&url).json(&body);
    if let Some(ref key) = state.api_key {
        req = req.bearer_auth(key);
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!(rule = %violation.rule_id, "inbox alert sent");
        }
        Ok(resp) => {
            tracing::warn!(
                rule = %violation.rule_id,
                status = %resp.status(),
                "inbox alert failed"
            );
        }
        Err(e) => {
            tracing::warn!(rule = %violation.rule_id, error = %e, "inbox alert unreachable");
        }
    }
}

async fn send_axon(state: &SupervisorState, violation: &Violation) {
    let url = format!("{}/axon/publish", state.kleos_url);

    let body = json!({
        "topic": "eidolon:supervisor",
        "event_type": "violation",
        "payload": {
            "rule_id": violation.rule_id,
            "severity": format!("{:?}", violation.severity),
            "message": violation.message,
            "context": violation.context,
        },
    });

    let mut req = state.client.post(&url).json(&body);
    if let Some(ref key) = state.api_key {
        req = req.bearer_auth(key);
    }

    match req.send().await {
        Ok(_) => {
            tracing::debug!(rule = %violation.rule_id, "axon event published");
        }
        Err(e) => {
            tracing::warn!(rule = %violation.rule_id, error = %e, "axon publish failed");
        }
    }
}

fn severity_to_importance(severity: &crate::checks::Severity) -> u8 {
    match severity {
        crate::checks::Severity::Critical => 9,
        crate::checks::Severity::Warning => 6,
        crate::checks::Severity::Info => 3,
    }
}
