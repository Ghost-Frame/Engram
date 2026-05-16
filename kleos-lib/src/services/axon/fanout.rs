//! Webhook fan-out for Axon event delivery.
//!
//! Queries subscriptions for a given channel and event type, then delivers
//! event payloads to registered webhook URLs via fire-and-forget HTTP POST.

use crate::db::Database;
use crate::{EngError, Result};

/// A resolved webhook delivery target extracted from an `axon_subscriptions` row.
#[derive(Debug)]
pub struct WebhookTarget {
    /// The agent name that registered this subscription.
    pub agent: String,
    /// The URL to POST the event payload to.
    pub webhook_url: String,
    /// Optional event-type filter. `None` means the subscription matches all event types.
    pub filter_type: Option<String>,
}

/// Maps a `rusqlite::Error` to an [`EngError::DatabaseMessage`].
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Queries `axon_subscriptions` for webhook targets matching a channel and event type.
///
/// Returns all subscriptions for `channel` that have a non-NULL `webhook_url`.
/// Subscriptions with a `filter_type` are only included when `filter_type` matches
/// `event_type`. Subscriptions with no `filter_type` (NULL) match all event types.
#[tracing::instrument(skip(db), fields(channel = %channel, event_type = %event_type))]
pub async fn get_webhook_targets(
    db: &Database,
    channel: &str,
    event_type: &str,
) -> Result<Vec<WebhookTarget>> {
    let channel_s = channel.to_string();
    let event_type_s = event_type.to_string();

    db.read(move |conn| {
        let sql = "SELECT agent, webhook_url, filter_type FROM axon_subscriptions \
                   WHERE channel = ?1 AND webhook_url IS NOT NULL";
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![channel_s])
            .map_err(rusqlite_to_eng_error)?;

        let mut targets = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let agent: String = row.get(0).map_err(rusqlite_to_eng_error)?;
            let webhook_url: String = row.get(1).map_err(rusqlite_to_eng_error)?;
            let filter_type: Option<String> = row.get(2).map_err(rusqlite_to_eng_error)?;

            // Skip subscriptions whose filter_type does not match the event type.
            if let Some(ref ft) = filter_type {
                if ft != &event_type_s {
                    continue;
                }
            }

            targets.push(WebhookTarget {
                agent,
                webhook_url,
                filter_type,
            });
        }
        Ok(targets)
    })
    .await
}

/// Delivers `event_json` to each target's webhook URL via fire-and-forget HTTP POST.
///
/// Spawns a Tokio task per target. Failures are logged as warnings and do not
/// propagate -- delivery is best-effort. The reqwest client uses a 5-second timeout.
pub fn deliver_webhooks(targets: &[WebhookTarget], event_json: &serde_json::Value) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    for target in targets {
        let client = client.clone();
        let url = target.webhook_url.clone();
        let agent = target.agent.clone();
        let body = event_json.clone();

        tokio::spawn(async move {
            match client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        tracing::warn!(
                            agent = %agent,
                            url = %url,
                            status = %resp.status(),
                            "webhook delivery returned non-2xx status"
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        agent = %agent,
                        url = %url,
                        error = %err,
                        "webhook delivery failed"
                    );
                }
            }
        });
    }
}

/// Publishes an event and fans out to all matching webhook subscribers.
///
/// Convenience wrapper that calls [`super::core::publish_event`], then
/// [`get_webhook_targets`], then [`deliver_webhooks`]. Returns the published event.
#[tracing::instrument(skip(db, req), fields(channel = %req.channel, action = %req.action))]
pub async fn publish_and_fanout(
    db: &Database,
    req: super::core::PublishEventRequest,
) -> crate::Result<super::core::Event> {
    let event = super::core::publish_event(db, req).await?;

    let event_json = serde_json::to_value(&event)?;
    let targets = get_webhook_targets(db, &event.channel, &event.action).await?;
    deliver_webhooks(&targets, &event_json);

    Ok(event)
}

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::services::axon::core::{upsert_subscription, SubscribeRequest};

    /// Sets up an in-memory database for testing.
    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    /// Verifies that `get_webhook_targets` correctly applies `filter_type` filtering:
    /// - A subscription with `filter_type = "task.completed"` matches only that event type.
    /// - A subscription with no `filter_type` matches all event types.
    #[tokio::test]
    async fn get_targets_filters_by_type() {
        let db = setup().await;

        // broca: only receives task.completed
        upsert_subscription(
            &db,
            SubscribeRequest {
                agent: "broca".into(),
                channel: "tasks".into(),
                filter_type: Some("task.completed".into()),
                webhook_url: Some("http://localhost:5000/ingest".into()),
            },
            1,
        )
        .await
        .expect("upsert broca");

        // logger: receives all event types on this channel
        upsert_subscription(
            &db,
            SubscribeRequest {
                agent: "logger".into(),
                channel: "tasks".into(),
                filter_type: None,
                webhook_url: Some("http://localhost:6000/hook".into()),
            },
            1,
        )
        .await
        .expect("upsert logger");

        // Both subscriptions should match "task.completed"
        let targets_completed = get_webhook_targets(&db, "tasks", "task.completed")
            .await
            .expect("get targets completed");
        assert_eq!(
            targets_completed.len(),
            2,
            "expected 2 targets for task.completed, got {:?}",
            targets_completed
                .iter()
                .map(|t| &t.agent)
                .collect::<Vec<_>>()
        );

        // Only logger should match "task.started" (broca's filter doesn't match)
        let targets_started = get_webhook_targets(&db, "tasks", "task.started")
            .await
            .expect("get targets started");
        assert_eq!(
            targets_started.len(),
            1,
            "expected 1 target for task.started, got {:?}",
            targets_started.iter().map(|t| &t.agent).collect::<Vec<_>>()
        );
        assert_eq!(targets_started[0].agent, "logger");
    }
}
