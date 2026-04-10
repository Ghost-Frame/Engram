// ============================================================================
// PAGERANK -- iterative weighted PageRank for memory graph
// ============================================================================

use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;

fn edge_weight(link_type: &str, similarity: f64) -> f64 {
    let tw = match link_type {
        "caused_by" | "causes" => 2.0,
        "updates" | "corrects" => 1.5,
        "extends" | "contradicts" => 1.3,
        "consolidates" => 0.5,
        _ => 1.0,
    };
    similarity * tw
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRankResult {
    pub scores: HashMap<i64, f64>,
    pub iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRankUpdateResult {
    pub memories: usize,
    pub iterations: u32,
}

pub async fn compute_pagerank(
    db: &Database, user_id: i64, damping: f64, max_iterations: u32,
) -> Result<PageRankResult> {
    let conn = db.connection();
    let mut mem_rows = conn.query(
        "SELECT id FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1",
        libsql::params![user_id]).await?;
    let mut memory_ids: Vec<i64> = Vec::new();
    while let Some(row) = mem_rows.next().await? { memory_ids.push(row.get(0)?); }

    let mut edge_rows = conn.query(
        "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
         FROM memory_links ml \
         JOIN memories ms ON ms.id = ml.source_id \
         JOIN memories mt ON mt.id = ml.target_id \
         WHERE ms.user_id = ?1 AND mt.user_id = ?1 \
           AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
           AND ms.is_archived = 0 AND mt.is_archived = 0",
        libsql::params![user_id]).await?;

    let n = memory_ids.len();
    if n == 0 { return Ok(PageRankResult { scores: HashMap::new(), iterations: 0 }); }

    let mut pr: HashMap<i64, f64> = HashMap::new();
    let mut out_w: HashMap<i64, f64> = HashMap::new();
    let mut in_links: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();
    let mem_set: std::collections::HashSet<i64> = memory_ids.iter().copied().collect();

    for &id in &memory_ids {
        pr.insert(id, 1.0 / n as f64);
        out_w.insert(id, 0.0);
        in_links.insert(id, Vec::new());
    }

    while let Some(row) = edge_rows.next().await? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let similarity: f64 = row.get(2)?;
        let link_type: String = row.get(3)?;
        if !mem_set.contains(&source_id) || !mem_set.contains(&target_id) { continue; }
        let w = edge_weight(&link_type, similarity);
        *out_w.entry(source_id).or_insert(0.0) += w;
        in_links.entry(target_id).or_default().push((source_id, w));
    }

    let mut converged_at = max_iterations;
    for iter in 0..max_iterations {
        let mut max_delta: f64 = 0.0;
        let mut new_pr: HashMap<i64, f64> = HashMap::new();
        for &id in &memory_ids {
            let incoming = in_links.get(&id).cloned().unwrap_or_default();
            let mut sum = 0.0;
            for (from_id, weight) in &incoming {
                let from_rank = pr.get(from_id).copied().unwrap_or(0.0);
                let from_out = out_w.get(from_id).copied().unwrap_or(1.0);
                sum += (from_rank * weight) / from_out;
            }
            let rank = (1.0 - damping) / n as f64 + damping * sum;
            new_pr.insert(id, rank);
            let delta = (rank - pr.get(&id).copied().unwrap_or(0.0)).abs();
            if delta > max_delta { max_delta = delta; }
        }
        for (id, rank) in &new_pr { pr.insert(*id, *rank); }
        if max_delta < 1e-6 { converged_at = iter + 1; break; }
    }

    Ok(PageRankResult { scores: pr, iterations: converged_at })
}

pub async fn update_pagerank_scores(db: &Database, user_id: i64) -> Result<PageRankUpdateResult> {
    let result = compute_pagerank(db, user_id, 0.85, 25).await?;
    if result.scores.is_empty() { return Ok(PageRankUpdateResult { memories: 0, iterations: result.iterations }); }

    let mut max_rank: f64 = 0.0;
    for &rank in result.scores.values() { if rank > max_rank { max_rank = rank; } }
    if max_rank == 0.0 { return Ok(PageRankUpdateResult { memories: result.scores.len(), iterations: result.iterations }); }

    let conn = db.connection();
    for (&id, &rank) in &result.scores {
        let normalized = rank / max_rank;
        conn.execute(
            "UPDATE memories SET pagerank_score = ?1 WHERE id = ?2 AND user_id = ?3",
            libsql::params![normalized, id, user_id],
        ).await?;
    }

    info!(user_id, memories = result.scores.len(), iterations = result.iterations,
        max_raw = format!("{:.6}", max_rank).as_str(), "pagerank_updated");
    Ok(PageRankUpdateResult { memories: result.scores.len(), iterations: result.iterations })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edge_weight() {
        assert!((edge_weight("caused_by", 0.5) - 1.0).abs() < 1e-10);
        assert!((edge_weight("related", 0.8) - 0.8).abs() < 1e-10);
        assert!((edge_weight("consolidates", 1.0) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_pagerank_in_memory() {
        let mut pr: HashMap<i64, f64> = HashMap::new();
        pr.insert(1, 1.0/3.0); pr.insert(2, 1.0/3.0); pr.insert(3, 1.0/3.0);
        let mut out_w: HashMap<i64, f64> = HashMap::new();
        out_w.insert(1, 1.0); out_w.insert(2, 1.0); out_w.insert(3, 0.0);
        let mut in_links: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();
        in_links.insert(1, vec![]); in_links.insert(2, vec![(1, 1.0)]); in_links.insert(3, vec![(2, 1.0)]);
        let n = 3.0_f64; let damping = 0.85;
        for _ in 0..25 {
            let mut new_pr: HashMap<i64, f64> = HashMap::new();
            for &id in &[1, 2, 3] {
                let incoming = in_links.get(&id).unwrap();
                let mut sum = 0.0;
                for (from_id, w) in incoming {
                    sum += (pr[from_id] * w) / out_w[from_id];
                }
                new_pr.insert(id, (1.0 - damping) / n + damping * sum);
            }
            pr = new_pr;
        }
        assert!(pr[&3] > pr[&1]);
    }
}
