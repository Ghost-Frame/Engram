//! Memory health diagnostics -- aggregate statistics about a user's memory store.

use super::types::MemoryHealthReport;
use crate::db::Database;
use crate::{EngError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Generate a health report for a user's memory store.
#[tracing::instrument(skip(db))]
pub async fn memory_health(db: &Database, user_id: i64) -> Result<MemoryHealthReport> {
    let report = db
        .read(move |conn| {
            // Total active memories
            let total: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
                    rusqlite::params![user_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(rusqlite_to_eng_error)?
                .unwrap_or(0);

            // Without embeddings
            let no_emb: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND embedding_vec_1024 IS NULL",
                    rusqlite::params![user_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(rusqlite_to_eng_error)?
                .unwrap_or(0);

            // Archived
            let archived: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_archived = 1",
                    rusqlite::params![user_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(rusqlite_to_eng_error)?
                .unwrap_or(0);

            // Superseded
            let superseded: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_superseded = 1",
                    rusqlite::params![user_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(rusqlite_to_eng_error)?
                .unwrap_or(0);

            // With links
            let with_links: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT ml.source_id) FROM memory_links ml \
                     JOIN memories m ON m.id = ml.source_id \
                     WHERE m.user_id = ?1 AND m.is_forgotten = 0",
                    rusqlite::params![user_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .map_err(rusqlite_to_eng_error)?
                .unwrap_or(0);

            // Average importance
            let avg_importance: f64 = conn
                .query_row(
                    "SELECT AVG(importance) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
                    rusqlite::params![user_id],
                    |row| row.get::<_, Option<f64>>(0),
                )
                .map_err(rusqlite_to_eng_error)?
                .unwrap_or(0.0);

            // Oldest memory
            let oldest: Option<String> = conn
                .query_row(
                    "SELECT MIN(created_at) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
                    rusqlite::params![user_id],
                    |row| row.get::<_, Option<String>>(0),
                )
                .map_err(rusqlite_to_eng_error)?;

            let coverage = if total > 0 {
                ((total - no_emb) as f64 / total as f64 * 100.0 * 100.0).round() / 100.0
            } else {
                0.0
            };

            Ok(MemoryHealthReport {
                total_memories: total,
                without_embeddings: no_emb,
                archived,
                superseded,
                with_links,
                avg_importance,
                oldest_memory: oldest,
                embedding_coverage_pct: coverage,
            })
        })
        .await?;

    Ok(report)
}
