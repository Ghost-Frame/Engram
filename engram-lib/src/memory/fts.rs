use super::types::FtsHit;
use crate::db::Database;
use crate::EngError;
use crate::Result;
use tracing::warn;

/// Sanitize a query string for FTS5 (remove special chars that break FTS syntax).
pub fn sanitize_fts_query(query: &str) -> String {
    // Remove FTS5 operators and special chars, keep alphanumeric and spaces
    let sanitized: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    // Split into tokens, filter short ones, join with spaces (implicit AND)
    sanitized
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Maximum FTS query length in bytes. Queries beyond this are rejected to
/// prevent denial-of-service through pathological FTS5 expressions.
use crate::validation::MAX_FTS_QUERY_LEN;

/// Search memories using FTS5 full-text search with BM25 ranking.
/// Returns up to `limit` results ordered by relevance (most relevant first).
#[tracing::instrument(skip(db, query), fields(query_len = query.len(), limit, user_id))]
pub async fn fts_search(
    db: &Database,
    query: &str,
    limit: usize,
    user_id: i64,
) -> Result<Vec<FtsHit>> {
    // SECURITY (SEC-MED-9): reject oversized queries before sanitization to
    // avoid CPU-intensive tokenisation on pathologically large input.
    if query.len() > MAX_FTS_QUERY_LEN {
        return Err(EngError::InvalidInput(format!(
            "query exceeds maximum length of {} bytes",
            MAX_FTS_QUERY_LEN
        )));
    }
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
          AND m.is_consolidated = 0
          AND m.user_id = ?2
        ORDER BY memories_fts.rank
        LIMIT ?3
    ";

    match db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            let mut rows = stmt
                .query(rusqlite::params![sanitized, user_id, limit as i64])
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

            // 6.9 capacity hint: LIMIT bounds the row count.
            let mut hits = Vec::with_capacity(limit);
            let mut pos: usize = 0;
            while let Some(row) = rows
                .next()
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            {
                let memory_id: i64 = row
                    .get(0)
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                let bm25_score: f64 = row
                    .get(1)
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                hits.push(FtsHit {
                    memory_id,
                    rank: pos,
                    bm25_score,
                });
                pos += 1;
            }

            Ok(hits)
        })
        .await
    {
        Ok(hits) => Ok(hits),
        Err(e) => {
            warn!("fts search failed: {}", e);
            Ok(vec![])
        }
    }
}
