use crate::db::Database;
use crate::Result;
use tracing::warn;

/// Result from FTS5 search -- id, rank position, and BM25 score
#[derive(Debug, Clone)]
pub struct FtsHit {
    pub memory_id: i64,
    pub rank: usize,
    pub bm25_score: f64,
}

/// Sanitize a query string for FTS5 (remove special chars that break FTS syntax).
fn sanitize_fts_query(query: &str) -> String {
    // Remove FTS5 operators and special chars, keep alphanumeric and spaces
    let sanitized: String = query
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { ' ' })
        .collect();
    // Split into tokens, filter short ones, join with spaces (implicit AND)
    sanitized
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Search memories using FTS5 full-text search with BM25 ranking.
/// Returns up to `limit` results ordered by relevance (most relevant first).
pub async fn fts_search(
    db: &Database,
    query: &str,
    limit: usize,
    user_id: i64,
) -> Result<Vec<FtsHit>> {
    let conn = db.connection();
    let sanitized = sanitize_fts_query(query);
    if sanitized.is_empty() {
        return Ok(vec![]);
    }

    // FTS5 match query joined with memories for user/forgotten filtering.
    // The built-in rank column returns negative scores (more negative = more relevant).
    // We negate it so bm25_score is positive and larger = more relevant.
    let sql = "
        SELECT m.id, -memories_fts.rank as bm25_score
        FROM memories_fts
        JOIN memories m ON m.id = memories_fts.rowid
        WHERE memories_fts MATCH ?1
          AND m.is_forgotten = 0
          AND m.is_latest = 1
          AND m.user_id = ?2
        ORDER BY memories_fts.rank
        LIMIT ?3
    ";

    let mut rows = match conn
        .query(sql, libsql::params![sanitized, user_id, limit as i64])
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("fts search failed: {}", e);
            return Ok(vec![]);
        }
    };

    let mut hits = Vec::new();
    let mut pos: usize = 0;
    while let Some(row) = rows.next().await? {
        let memory_id: i64 = row.get(0)?;
        let bm25_score: f64 = row.get(1)?;
        hits.push(FtsHit {
            memory_id,
            rank: pos,
            bm25_score,
        });
        pos += 1;
    }

    Ok(hits)
}
