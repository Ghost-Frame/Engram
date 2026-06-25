//! Event retention -- prunes expired events per channel's `retain_hours` setting.
//!
//! Each channel in `axon_channels` carries a `retain_hours` value (default 168,
//! which is 7 days). `prune_expired_events` iterates all channels and deletes
//! events older than that window. The entire operation runs inside a single
//! `db.write()` call so it holds exactly one write connection.

use crate::db::Database;
use crate::Result;

/// Prune expired events from every channel according to each channel's
/// `retain_hours` value.
///
/// Events whose `created_at` timestamp is older than `retain_hours` hours ago
/// are deleted. The function returns the total number of rows deleted across
/// all channels.
///
/// The entire operation -- reading channel rows and performing all deletes --
/// runs inside a single `db.write()` call to avoid opening multiple write
/// transactions and to minimise lock contention.
pub async fn prune_expired_events(db: &Database) -> Result<usize> {
    db.write(move |conn| {
        // Phase 1: map each registered channel to its retain_hours. rusqlite
        // does not allow two active statements on the same connection, so each
        // query is fully drained into an owned collection before the next.
        let configured: std::collections::HashMap<String, i64> = {
            let mut stmt = conn.prepare("SELECT name, retain_hours FROM axon_channels")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?;
            let mut map = std::collections::HashMap::new();
            for row in rows {
                let (name, hours) = row?;
                map.insert(name, hours);
            }
            map
        };

        // Phase 1b: enumerate the channels that actually have events. Events can
        // be published to a channel that was never registered via ensure_channel
        // (publish_event does no existence check), and those would otherwise grow
        // unbounded because the configured-channel loop never matched them.
        let event_channels: Vec<String> = {
            let mut stmt = conn.prepare("SELECT DISTINCT channel FROM axon_events")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut collected = Vec::new();
            for row in rows {
                collected.push(row?);
            }
            collected
        };

        // Phase 2: delete expired events for each channel that has any, using the
        // configured retention or the default window for unregistered channels.
        let mut total_deleted: usize = 0;
        for channel in event_channels {
            let retain_hours = configured
                .get(&channel)
                .copied()
                .unwrap_or(DEFAULT_RETAIN_HOURS);
            let interval = format!("-{} hours", retain_hours);
            let deleted = conn.execute(
                "DELETE FROM axon_events WHERE channel = ?1 AND created_at < datetime('now', ?2)",
                rusqlite::params![channel, interval],
            )?;
            total_deleted += deleted;
        }

        Ok(total_deleted)
    })
    .await
}

/// Default retention window (7 days) applied to channels that have events but
/// no `axon_channels` row -- mirrors the schema default for `retain_hours`.
const DEFAULT_RETAIN_HOURS: i64 = 168;

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::services::axon::core::{publish_event, PublishEventRequest};

    /// Build a minimal publish request targeting the seeded `system` channel.
    fn test_publish_req() -> PublishEventRequest {
        PublishEventRequest {
            channel: "system".into(),
            action: "test".into(),
            payload: None,
            source: Some("test".into()),
            agent: None,
            user_id: Some(1),
        }
    }

    /// Publish an event, backdate it to 200 hours ago (past the 168-hour default
    /// retention window), then assert that `prune_expired_events` removes it.
    #[tokio::test]
    async fn prune_removes_old_events() {
        let db = Database::connect_memory().await.expect("db");

        let ev = publish_event(&db, test_publish_req())
            .await
            .expect("publish");

        // Backdate the event past the retention window.
        db.write(move |conn| {
            conn.execute(
                "UPDATE axon_events SET created_at = datetime('now', '-200 hours') WHERE id = ?1",
                rusqlite::params![ev.id],
            )?;
            Ok(())
        })
        .await
        .expect("backdate");

        let deleted = prune_expired_events(&db).await.expect("prune");
        assert_eq!(
            deleted, 1,
            "expected exactly one expired event to be pruned"
        );
    }

    /// Publish a recent event (not backdated) and assert that
    /// `prune_expired_events` leaves it untouched.
    #[tokio::test]
    async fn prune_keeps_recent_events() {
        let db = Database::connect_memory().await.expect("db");

        let _ev = publish_event(&db, test_publish_req())
            .await
            .expect("publish");

        let deleted = prune_expired_events(&db).await.expect("prune");
        assert_eq!(
            deleted, 0,
            "expected no events to be pruned for a fresh event"
        );
    }

    /// An event on a channel that was never registered in axon_channels must
    /// still be pruned via the default retention window (previously such events
    /// grew unbounded because the prune loop only iterated registered channels).
    #[tokio::test]
    async fn prune_removes_old_events_on_unregistered_channel() {
        let db = Database::connect_memory().await.expect("db");

        let mut req = test_publish_req();
        req.channel = "ad-hoc-never-registered".into();
        let ev = publish_event(&db, req).await.expect("publish");

        db.write(move |conn| {
            conn.execute(
                "UPDATE axon_events SET created_at = datetime('now', '-200 hours') WHERE id = ?1",
                rusqlite::params![ev.id],
            )?;
            Ok(())
        })
        .await
        .expect("backdate");

        let deleted = prune_expired_events(&db).await.expect("prune");
        assert_eq!(
            deleted, 1,
            "expired event on unregistered channel must be pruned"
        );
    }
}
