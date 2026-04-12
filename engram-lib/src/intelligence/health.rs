//! Memory health diagnostics -- aggregate statistics about a user's memory store.

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryHealthReport {
    pub total_memories: i64,
    pub without_embeddings: i64,
    pub archived: i64,
    pub superseded: i64,
    pub with_links: i64,
    pub avg_importance: f64,
    pub oldest_memory: Option<String>,
    pub embedding_coverage_pct: f64,
}

/// Generate a health report for a user's memory store.
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
