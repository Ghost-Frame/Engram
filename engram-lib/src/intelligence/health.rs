//! Memory health diagnostics -- aggregate statistics about a user's memory store.

use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

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
    let conn = db.connection();

    // Total active memories
    let mut r = conn
        .query(
            "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
            libsql::params![user_id],
        )
        .await?;
    let total: i64 = match r.next().await? {
        Some(row) => row.get::<Option<i64>>(0)?.unwrap_or(0),
        None => 0,
    };

    // Without embeddings
    let mut r = conn
        .query(
            "SELECT COUNT(*) FROM memories \
             WHERE user_id = ?1 AND is_forgotten = 0 AND embedding_vec_1024 IS NULL",
            libsql::params![user_id],
        )
        .await?;
    let no_emb: i64 = match r.next().await? {
        Some(row) => row.get::<Option<i64>>(0)?.unwrap_or(0),
        None => 0,
    };

    // Archived
    let mut r = conn
        .query(
            "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_archived = 1",
            libsql::params![user_id],
        )
        .await?;
    let archived: i64 = match r.next().await? {
        Some(row) => row.get::<Option<i64>>(0)?.unwrap_or(0),
        None => 0,
    };

    // Superseded
    let mut r = conn
        .query(
            "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_superseded = 1",
            libsql::params![user_id],
        )
        .await?;
    let superseded: i64 = match r.next().await? {
        Some(row) => row.get::<Option<i64>>(0)?.unwrap_or(0),
        None => 0,
    };

    // With links
    let mut r = conn
        .query(
            "SELECT COUNT(DISTINCT ml.source_id) FROM memory_links ml \
             JOIN memories m ON m.id = ml.source_id \
             WHERE m.user_id = ?1 AND m.is_forgotten = 0",
            libsql::params![user_id],
        )
        .await?;
    let with_links: i64 = match r.next().await? {
        Some(row) => row.get::<Option<i64>>(0)?.unwrap_or(0),
        None => 0,
    };

    // Average importance
    let mut r = conn
        .query(
            "SELECT AVG(importance) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
            libsql::params![user_id],
        )
        .await?;
    let avg_importance: f64 = match r.next().await? {
        Some(row) => row.get::<Option<f64>>(0)?.unwrap_or(0.0),
        None => 0.0,
    };

    // Oldest memory
    let mut r = conn
        .query(
            "SELECT MIN(created_at) FROM memories WHERE user_id = ?1 AND is_forgotten = 0",
            libsql::params![user_id],
        )
        .await?;
    let oldest: Option<String> = match r.next().await? {
        Some(row) => row.get(0)?,
        None => None,
    };

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
}
