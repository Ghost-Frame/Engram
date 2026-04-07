use crate::db::Database;
use crate::memory::types::{Memory, SearchRequest, SearchResult};
use crate::Result;
use super::vector::vector_search;
use super::fts::fts_search;
use super::{row_to_memory, MEMORY_COLUMNS};
use tracing::info;
use std::collections::HashMap;

/// RRF constant k=60 from Cormack et al. 2009
const RRF_K: f64 = 60.0;

/// Default search limit
const DEFAULT_LIMIT: usize = 10;

/// Run a hybrid search combining vector ANN and FTS5, fused with RRF.
/// Both channels are best-effort -- if one fails, the other still contributes.
pub async fn hybrid_search(db: &Database, req: SearchRequest) -> Result<Vec<SearchResult>> {
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT);
    let user_id = req.user_id.unwrap_or(1);
    // Fetch more candidates for fusion than the final limit
    let candidate_limit = (limit * 3).max(30).min(200);

    // Collect RRF scores: memory_id -> cumulative RRF score
    let mut rrf_scores: HashMap<i64, f64> = HashMap::new();
    // Track which channels found each result
    let mut channels: HashMap<i64, Vec<&'static str>> = HashMap::new();

    // Channel 1: Vector ANN search (only if embedding is provided)
    if let Some(ref embedding) = req.embedding {
        let vector_hits = vector_search(db, embedding, candidate_limit, user_id).await?;
        for hit in &vector_hits {
            let rrf = 1.0 / (RRF_K + hit.rank as f64 + 1.0);
            *rrf_scores.entry(hit.memory_id).or_default() += rrf;
            channels.entry(hit.memory_id).or_default().push("vector");
        }
    }

    // Channel 2: FTS5 BM25 search (only if query is non-empty)
    if !req.query.is_empty() {
        let fts_hits = fts_search(db, &req.query, candidate_limit, user_id).await?;
        for hit in &fts_hits {
            let rrf = 1.0 / (RRF_K + hit.rank as f64 + 1.0);
            *rrf_scores.entry(hit.memory_id).or_default() += rrf;
            channels.entry(hit.memory_id).or_default().push("fts");
        }
    }

    if rrf_scores.is_empty() {
        return Ok(vec![]);
    }

    // Sort candidates by RRF score descending, take top `limit`
    let mut candidates: Vec<(i64, f64)> = rrf_scores.into_iter().collect();
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(limit);

    // Fetch full Memory objects for top candidates and apply decay scoring
    let conn = db.connection();
    let fetch_sql = format!("SELECT {} FROM memories WHERE id = ?1", MEMORY_COLUMNS);
    let mut results: Vec<SearchResult> = Vec::with_capacity(candidates.len());

    for (memory_id, rrf_score) in &candidates {
        let mut rows = match conn.query(&fetch_sql, libsql::params![*memory_id]).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("failed to fetch memory {}: {}", memory_id, e);
                continue;
            }
        };

        if let Some(row) = rows.next().await? {
            let memory: Memory = row_to_memory(&row)?;

            // Apply FSRS decay to adjust relevance score
            let decay = crate::fsrs::decay::calculate_decay_score(
                memory.importance as f32,
                &memory.created_at,
                memory.access_count,
                memory.last_accessed_at.as_deref(),
                memory.is_static,
                memory.source_count,
                memory.fsrs_stability.map(|s| s as f32),
            );

            // Normalize decay to 0-1 range (max importance is 10)
            let decay_factor = (decay / 10.0).max(0.01) as f64;
            let final_score = rrf_score * decay_factor;

            let ch = channels
                .get(memory_id)
                .map(|c| c.join("+"))
                .unwrap_or_else(|| "unknown".to_string());

            results.push(SearchResult {
                memory,
                score: final_score,
                search_type: ch,
            });
        }
    }

    // Re-sort by final score after decay adjustment
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    info!(
        query_len = req.query.len(),
        results = results.len(),
        "hybrid search completed"
    );

    Ok(results)
}
