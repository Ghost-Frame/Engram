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

/// Build an OR-of-tokens FTS5 MATCH expression for memory search.
///
/// Space-joined tokens (see `sanitize_fts_query`) are an implicit AND in FTS5, so a
/// multi-term natural-language query returns zero hits unless every stem co-occurs in
/// one document. That collapses hybrid search to vector-only whenever one term is
/// missing. Memory search instead ORs the tokens, so partial matches surface while
/// BM25 still ranks documents that match more terms higher. Each token is
/// alphanumeric-only (special chars already mapped to spaces) and wrapped as a quoted
/// phrase so that FTS5 boolean keywords appearing inside the user query
/// (AND/OR/NOT/NEAR) cannot be reinterpreted as operators. Returns an empty string
/// when no usable token remains, matching `sanitize_fts_query`'s contract.
pub fn fts_or_match_query(query: &str) -> String {
    // Same character sanitisation as sanitize_fts_query: keep alphanumerics and
    // whitespace, replace every other character with a space.
    let cleaned: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect();
    // Quote each token and join with the OR operator.
    cleaned
        .split_whitespace()
        .filter(|w| w.len() >= 2)
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
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
    // Memory search ORs the tokens (not the implicit-AND of sanitize_fts_query) so a
    // multi-term query does not zero out when one term is absent from a document.
    let sanitized = fts_or_match_query(query);
    if sanitized.is_empty() {
        return Ok(vec![]);
    }

    // FTS5 match query joined with memories for user/forgotten filtering.
    // The built-in rank column returns negative scores (more negative = more relevant).
    // We negate it so bm25_score is positive and larger = more relevant.
    // The owner predicate (?2) keeps single-DB (shared) mode from returning
    // another user's full-text hits; a no-op in a single-owner shard.
    let sql = "
        SELECT m.id, -memories_fts.rank as bm25_score
        FROM memories_fts
        JOIN memories m ON m.id = memories_fts.rowid
        WHERE memories_fts MATCH ?1
          AND m.user_id = ?2
          AND m.is_forgotten = 0
          AND m.is_latest = 1
        ORDER BY memories_fts.rank
        LIMIT ?3
    ";

    match db
        .read(move |conn| {
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query(rusqlite::params![sanitized, user_id, limit as i64])?;

            // 6.9 capacity hint: LIMIT bounds the row count.
            let mut hits = Vec::with_capacity(limit);
            let mut pos: usize = 0;
            while let Some(row) = rows.next()? {
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
