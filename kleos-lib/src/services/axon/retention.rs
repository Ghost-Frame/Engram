//! Event retention -- prunes expired events per channel's `retain_hours` setting.
//!
//! Each channel in `axon_channels` carries a `retain_hours` value (default 168,
//! which is 7 days). `prune_expired_events` iterates all channels and deletes
//! events older than that window. The entire operation runs inside a single
//! `db.write()` call so it holds exactly one write connection.

use crate::db::Database;
use crate::{EngError, Result};

/// Convert a `rusqlite::Error` into an `EngError`.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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
        // Phase 1: collect all (name, retain_hours) pairs into a Vec so we can
        // drop the statement before opening any DELETE statements. rusqlite
        // does not allow two active statements on the same connection.
        let channels: Vec<(String, i64)> = {
            let mut stmt = conn
                .prepare("SELECT name, retain_hours FROM axon_channels")
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map([], |row| {
                    let name: String = row.get(0)?;
                    let retain_hours: i64 = row.get(1)?;
                    Ok((name, retain_hours))
                })
                .map_err(rusqlite_to_eng_error)?;

            let mut collected = Vec::new();
            for row in rows {
                collected.push(row.map_err(rusqlite_to_eng_error)?);
            }
            collected
            // `stmt` is dropped here before Phase 2
        };

        // Phase 2: delete expired events for each channel.
        let mut total_deleted: usize = 0;
        for (name, retain_hours) in channels {
            let interval = format!("-{} hours", retain_hours);
            let deleted = conn
                .execute(
                    "DELETE FROM axon_events WHERE channel = ?1 AND created_at < datetime('now', ?2)",
                    rusqlite::params![name, interval],
                )
                .map_err(rusqlite_to_eng_error)?;
            total_deleted += deleted;
        }

        Ok(total_deleted)
    })
    .await
}

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
            )
            .map_err(rusqlite_to_eng_error)?;
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
}
