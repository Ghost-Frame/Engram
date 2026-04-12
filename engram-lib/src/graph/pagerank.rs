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
    db: &Database,
    user_id: i64,
    damping: f64,
    max_iterations: u32,
) -> Result<PageRankResult> {
    let conn = db.connection();
    let mut mem_rows = conn.query(
        "SELECT id FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1",
        libsql::params![user_id]).await?;
    let mut memory_ids: Vec<i64> = Vec::new();
    while let Some(row) = mem_rows.next().await? {
        memory_ids.push(row.get(0)?);
    }

    // GROUP BY deduplicates multiple memory_links rows for the same
    // (source_id, target_id) pair. Duplicates would inflate out_w and
    // in_links, distorting PageRank scores (RB-L8).
    let mut edge_rows = conn
        .query(
            "SELECT ml.source_id, ml.target_id, MAX(ml.similarity), MAX(ml.type) \
         FROM memory_links ml \
         JOIN memories ms ON ms.id = ml.source_id \
         JOIN memories mt ON mt.id = ml.target_id \
         WHERE ms.user_id = ?1 AND mt.user_id = ?1 \
           AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
           AND ms.is_archived = 0 AND mt.is_archived = 0 \
         GROUP BY ml.source_id, ml.target_id",
            libsql::params![user_id],
        )
        .await?;

    let n = memory_ids.len();
    if n == 0 {
        return Ok(PageRankResult {
            scores: HashMap::new(),
            iterations: 0,
        });
    }

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
        if !mem_set.contains(&source_id) || !mem_set.contains(&target_id) {
            continue;
        }
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
            if delta > max_delta {
                max_delta = delta;
            }
        }
        for (id, rank) in &new_pr {
            pr.insert(*id, *rank);
        }
        if max_delta < 1e-6 {
            converged_at = iter + 1;
            break;
        }
    }

    Ok(PageRankResult {
        scores: pr,
        iterations: converged_at,
    })
}

pub async fn update_pagerank_scores(db: &Database, user_id: i64) -> Result<PageRankUpdateResult> {
    let result = compute_pagerank(db, user_id, 0.85, 25).await?;
    if result.scores.is_empty() {
        return Ok(PageRankUpdateResult {
            memories: 0,
            iterations: result.iterations,
        });
    }

    let mut max_rank: f64 = 0.0;
    for &rank in result.scores.values() {
        if rank > max_rank {
            max_rank = rank;
        }
    }
    if max_rank == 0.0 {
        return Ok(PageRankUpdateResult {
            memories: result.scores.len(),
            iterations: result.iterations,
        });
    }

    let conn = db.connection();
    for (&id, &rank) in &result.scores {
        let normalized = rank / max_rank;
        conn.execute(
            "UPDATE memories SET pagerank_score = ?1 WHERE id = ?2 AND user_id = ?3",
            libsql::params![normalized, id, user_id],
        )
        .await?;
    }

    info!(
        user_id,
        memories = result.scores.len(),
        iterations = result.iterations,
        max_raw = format!("{:.6}", max_rank).as_str(),
        "pagerank_updated"
    );
    Ok(PageRankUpdateResult {
        memories: result.scores.len(),
        iterations: result.iterations,
    })
}

/// Compute normalized PageRank scores for a user and return as a vec of (memory_id, score).
/// Does not write to any table.
pub async fn compute_pagerank_for_user(db: &Database, user_id: i64) -> Result<Vec<(i64, f64)>> {
    let result = compute_pagerank(db, user_id, 0.85, 25).await?;
    if result.scores.is_empty() {
        return Ok(Vec::new());
    }
    let max_rank = result.scores.values().copied().fold(0.0_f64, f64::max);
    if max_rank == 0.0 {
        return Ok(result.scores.into_keys().map(|id| (id, 0.0)).collect());
    }
    Ok(result
        .scores
        .into_iter()
        .map(|(id, score)| (id, score / max_rank))
        .collect())
}

/// Read the current dirty_count for a user. Returns 0 if no row exists.
///
/// Callers that want race-free dirty tracking should read this value BEFORE
/// starting a PageRank compute, then pass it to
/// [`persist_pagerank_with_snapshot`] so that only the counted mutations get
/// cleared. Any increments that arrived while the compute was running stay
/// behind, and the next refresh cycle picks them up.
pub async fn snapshot_pagerank_dirty(db: &Database, user_id: i64) -> Result<i64> {
    let mut rows = db
        .connection()
        .query(
            "SELECT dirty_count FROM pagerank_dirty WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get::<i64>(0)?),
        None => Ok(0),
    }
}

/// Upsert computed scores into `memory_pagerank` and subtract the supplied
/// dirty snapshot from the counter (clamped at zero). See
/// [`snapshot_pagerank_dirty`] for the usage pattern that avoids losing
/// concurrent writes.
pub async fn persist_pagerank_with_snapshot(
    db: &Database,
    user_id: i64,
    scores: &[(i64, f64)],
    dirty_snapshot: i64,
) -> Result<()> {
    let conn = db.connection();
    let now = chrono::Utc::now().timestamp();
    for &(memory_id, score) in scores {
        conn.execute(
            "INSERT INTO memory_pagerank (memory_id, user_id, score, computed_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(memory_id) DO UPDATE SET \
               score = excluded.score, \
               computed_at = excluded.computed_at",
            libsql::params![memory_id, user_id, score, now],
        )
        .await?;
    }
    // Subtract only the increments we compensated for. Any concurrent
    // mark_pagerank_dirty that fired while compute was running remains in
    // the counter and schedules the next refresh cycle. Clamped at 0 so a
    // spurious over-count cannot push the value negative.
    conn.execute(
        "INSERT INTO pagerank_dirty (user_id, dirty_count, last_refresh) \
         VALUES (?1, 0, ?2) \
         ON CONFLICT(user_id) DO UPDATE SET \
           dirty_count = MAX(0, dirty_count - ?3), \
           last_refresh = excluded.last_refresh",
        libsql::params![user_id, now, dirty_snapshot],
    )
    .await?;
    info!(
        user_id,
        scores = scores.len(),
        dirty_cleared = dirty_snapshot,
        "pagerank_persisted"
    );
    Ok(())
}

/// Upsert computed scores and reset the dirty counter. This takes the
/// snapshot internally, which is correct for callers that compute and
/// persist in a single await with no concurrent writers (admin rebuilds,
/// tests). Background refresh workers should use the explicit snapshot API.
pub async fn persist_pagerank(db: &Database, user_id: i64, scores: &[(i64, f64)]) -> Result<()> {
    let snapshot = snapshot_pagerank_dirty(db, user_id).await?;
    persist_pagerank_with_snapshot(db, user_id, scores, snapshot).await
}

/// Increment the dirty counter for a user. Called after memory/edge mutations.
pub async fn mark_pagerank_dirty(db: &Database, user_id: i64, delta: i64) -> Result<()> {
    db.connection()
        .execute(
            "INSERT INTO pagerank_dirty (user_id, dirty_count, last_refresh) \
             VALUES (?1, ?2, 0) \
             ON CONFLICT(user_id) DO UPDATE SET dirty_count = dirty_count + ?2",
            libsql::params![user_id, delta],
        )
        .await?;
    Ok(())
}

// ============================================================================
// INCREMENTAL PAGERANK -- delta updates without full recompute
// ============================================================================

const DAMPING: f64 = 0.85;
const CONVERGENCE_THRESHOLD: f64 = 1e-6;

/// Incremental PageRank update when a new memory is added.
/// Inserts the memory with base rank and does NOT trigger full recompute.
/// The new node has no incoming links yet, so it gets the teleportation score only.
pub async fn incremental_add_memory(db: &Database, memory_id: i64, user_id: i64) -> Result<()> {
    // Get current memory count to compute base rank
    let mut rows = db
        .connection()
        .query(
            "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
            libsql::params![user_id],
        )
        .await?;
    let n: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 1,
    };

    // Base rank for new node: (1-d)/N
    let base_rank = (1.0 - DAMPING) / n.max(1) as f64;

    let now = chrono::Utc::now().timestamp();
    db.connection()
        .execute(
            "INSERT INTO memory_pagerank (memory_id, user_id, score, computed_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(memory_id) DO UPDATE SET \
               score = excluded.score, computed_at = excluded.computed_at",
            libsql::params![memory_id, user_id, base_rank, now],
        )
        .await?;

    Ok(())
}

/// Incremental PageRank update when a link is added.
/// Propagates score changes locally to affected nodes (2-hop neighborhood).
pub async fn incremental_add_link(
    db: &Database,
    source_id: i64,
    target_id: i64,
    similarity: f64,
    link_type: &str,
    user_id: i64,
) -> Result<usize> {
    let conn = db.connection();

    // Get current scores for source and target
    let mut rows = conn
        .query(
            "SELECT memory_id, score FROM memory_pagerank WHERE memory_id IN (?1, ?2) AND user_id = ?3",
            libsql::params![source_id, target_id, user_id],
        )
        .await?;

    let mut scores: HashMap<i64, f64> = HashMap::new();
    while let Some(row) = rows.next().await? {
        let mid: i64 = row.get(0)?;
        let score: f64 = row.get(1)?;
        scores.insert(mid, score);
    }

    // If neither node has a score, initialize them
    if scores.is_empty() {
        incremental_add_memory(db, source_id, user_id).await?;
        incremental_add_memory(db, target_id, user_id).await?;
        return Ok(2);
    }

    let source_score = scores.get(&source_id).copied().unwrap_or(0.01);
    let weight = edge_weight(link_type, similarity);

    // Get source's total outgoing weight
    let mut out_rows = conn
        .query(
            "SELECT SUM(similarity) FROM memory_links WHERE source_id = ?1",
            libsql::params![source_id],
        )
        .await?;
    let total_out: f64 = match out_rows.next().await? {
        Some(row) => row.get::<Option<f64>>(0)?.unwrap_or(1.0),
        None => 1.0,
    };

    // Compute contribution from source to target
    let contribution = DAMPING * source_score * weight / total_out.max(weight);

    // Update target score
    let old_target = scores.get(&target_id).copied().unwrap_or(0.01);
    let new_target = old_target + contribution;

    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO memory_pagerank (memory_id, user_id, score, computed_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(memory_id) DO UPDATE SET \
           score = excluded.score, computed_at = excluded.computed_at",
        libsql::params![target_id, user_id, new_target, now],
    )
    .await?;

    // Propagate to target's neighbors (1-hop)
    let mut neighbor_rows = conn
        .query(
            "SELECT target_id, similarity, type FROM memory_links WHERE source_id = ?1",
            libsql::params![target_id],
        )
        .await?;

    let mut updated = 1usize;
    let delta = new_target - old_target;

    while let Some(row) = neighbor_rows.next().await? {
        let neighbor_id: i64 = row.get(0)?;
        let sim: f64 = row.get(1)?;
        let lt: String = row.get(2)?;
        let w = edge_weight(&lt, sim);
        let neighbor_contribution = DAMPING * delta * w / total_out.max(1.0);

        if neighbor_contribution.abs() > CONVERGENCE_THRESHOLD {
            conn.execute(
                "UPDATE memory_pagerank SET score = score + ?1, computed_at = ?2 \
                 WHERE memory_id = ?3 AND user_id = ?4",
                libsql::params![neighbor_contribution, now, neighbor_id, user_id],
            )
            .await?;
            updated += 1;
        }
    }

    info!(
        source_id,
        target_id,
        updated,
        contribution = format!("{:.6}", contribution).as_str(),
        "incremental_pagerank_link"
    );

    Ok(updated)
}

/// Incremental PageRank update when a memory is deleted.
/// Removes the score and redistributes to remaining nodes.
pub async fn incremental_remove_memory(db: &Database, memory_id: i64, user_id: i64) -> Result<()> {
    let conn = db.connection();

    // Get the score being removed
    let mut rows = conn
        .query(
            "SELECT score FROM memory_pagerank WHERE memory_id = ?1 AND user_id = ?2",
            libsql::params![memory_id, user_id],
        )
        .await?;

    let removed_score: f64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => return Ok(()), // No score to remove
    };

    // Delete the score
    conn.execute(
        "DELETE FROM memory_pagerank WHERE memory_id = ?1 AND user_id = ?2",
        libsql::params![memory_id, user_id],
    )
    .await?;

    // Get remaining memory count
    let mut count_rows = conn
        .query(
            "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    let remaining: i64 = match count_rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };

    if remaining > 0 {
        // Distribute removed score evenly (simplified redistribution)
        let redistribution = removed_score / remaining as f64;
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "UPDATE memory_pagerank SET score = score + ?1, computed_at = ?2 WHERE user_id = ?3",
            libsql::params![redistribution, now, user_id],
        )
        .await?;
    }

    info!(memory_id, removed_score = format!("{:.6}", removed_score).as_str(), "incremental_pagerank_remove");

    Ok(())
}

/// Check if incremental updates have drifted too far from true PageRank.
/// Returns true if a full recompute is recommended.
pub async fn needs_full_recompute(db: &Database, user_id: i64, drift_threshold: f64) -> Result<bool> {
    // Compare sum of incremental scores to expected sum (should be ~1.0)
    let mut rows = db
        .connection()
        .query(
            "SELECT SUM(score), COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => {
            let sum: f64 = row.get::<Option<f64>>(0)?.unwrap_or(0.0);
            let count: i64 = row.get(1)?;
            if count == 0 {
                return Ok(false);
            }
            // PageRank scores should sum to approximately 1.0
            // If drift exceeds threshold, recommend full recompute
            let drift = (sum - 1.0).abs();
            Ok(drift > drift_threshold)
        }
        None => Ok(false),
    }
}

// ============================================================================
// COMMUNITY-SCOPED PAGERANK -- compute per community for reduced memory usage
// ============================================================================

/// Compute PageRank for a single community only.
/// Much more memory-efficient than global compute for large graphs.
pub async fn compute_pagerank_for_community(
    db: &Database,
    user_id: i64,
    community_id: i64,
    damping: f64,
    max_iterations: u32,
) -> Result<PageRankResult> {
    let conn = db.connection();

    // Get memories in this community
    let mut mem_rows = conn
        .query(
            "SELECT id FROM memories \
             WHERE user_id = ?1 AND community_id = ?2 \
               AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1",
            libsql::params![user_id, community_id],
        )
        .await?;

    let mut memory_ids: Vec<i64> = Vec::new();
    while let Some(row) = mem_rows.next().await? {
        memory_ids.push(row.get(0)?);
    }

    let n = memory_ids.len();
    if n == 0 {
        return Ok(PageRankResult {
            scores: HashMap::new(),
            iterations: 0,
        });
    }

    // Build ID set for fast lookup
    let mem_set: std::collections::HashSet<i64> = memory_ids.iter().copied().collect();
    let id_list = memory_ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    // Get edges within this community
    let edge_sql = format!(
        "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
         FROM memory_links ml \
         WHERE ml.source_id IN ({id_list}) AND ml.target_id IN ({id_list})"
    );

    let mut edge_rows = conn.query(&edge_sql, ()).await?;

    let mut pr: HashMap<i64, f64> = HashMap::new();
    let mut out_w: HashMap<i64, f64> = HashMap::new();
    let mut in_links: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();

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

        if !mem_set.contains(&source_id) || !mem_set.contains(&target_id) {
            continue;
        }

        let w = edge_weight(&link_type, similarity);
        *out_w.entry(source_id).or_insert(0.0) += w;
        in_links.entry(target_id).or_default().push((source_id, w));
    }

    // Power iteration (same as global, but on smaller subgraph)
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
            if delta > max_delta {
                max_delta = delta;
            }
        }

        for (id, rank) in &new_pr {
            pr.insert(*id, *rank);
        }

        if max_delta < 1e-6 {
            converged_at = iter + 1;
            break;
        }
    }

    info!(
        user_id,
        community_id,
        memories = n,
        iterations = converged_at,
        "community_pagerank_computed"
    );

    Ok(PageRankResult {
        scores: pr,
        iterations: converged_at,
    })
}

/// Compute PageRank for all communities in parallel, then merge results.
/// Much more memory-efficient than loading entire graph at once.
pub async fn compute_pagerank_by_communities(
    db: &Database,
    user_id: i64,
) -> Result<Vec<(i64, f64)>> {
    let conn = db.connection();

    // Get all distinct community IDs for this user
    let mut comm_rows = conn
        .query(
            "SELECT DISTINCT community_id FROM memories \
             WHERE user_id = ?1 AND community_id IS NOT NULL \
               AND is_forgotten = 0 AND is_latest = 1",
            libsql::params![user_id],
        )
        .await?;

    let mut community_ids: Vec<i64> = Vec::new();
    while let Some(row) = comm_rows.next().await? {
        community_ids.push(row.get(0)?);
    }

    // Also handle memories without community (community_id IS NULL)
    let mut orphan_rows = conn
        .query(
            "SELECT id FROM memories \
             WHERE user_id = ?1 AND community_id IS NULL \
               AND is_forgotten = 0 AND is_latest = 1",
            libsql::params![user_id],
        )
        .await?;

    let mut orphan_ids: Vec<i64> = Vec::new();
    while let Some(row) = orphan_rows.next().await? {
        orphan_ids.push(row.get(0)?);
    }

    let mut all_scores: Vec<(i64, f64)> = Vec::new();

    // Compute per-community
    for cid in community_ids {
        let result = compute_pagerank_for_community(db, user_id, cid, 0.85, 25).await?;
        let max_score = result.scores.values().copied().fold(0.0_f64, f64::max);
        if max_score > 0.0 {
            for (mid, score) in result.scores {
                all_scores.push((mid, score / max_score));
            }
        }
    }

    // Give orphan memories base score
    let orphan_score = 0.1; // Low but non-zero
    for mid in orphan_ids {
        all_scores.push((mid, orphan_score));
    }

    info!(
        user_id,
        total_scores = all_scores.len(),
        "community_pagerank_merged"
    );

    Ok(all_scores)
}

/// Ensure the pagerank cache is populated for this user. If empty, runs a
/// synchronous compute and persists the result. Subsequent calls are cheap
/// (single COUNT query that returns early).
pub async fn ensure_pagerank_for_user(db: &Database, user_id: i64) -> Result<()> {
    let mut rows = db
        .connection()
        .query(
            "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1 LIMIT 1",
            libsql::params![user_id],
        )
        .await?;
    let count: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };
    if count == 0 {
        let scores = compute_pagerank_for_user(db, user_id).await?;
        if !scores.is_empty() {
            persist_pagerank(db, user_id, &scores).await?;
        }
    }
    Ok(())
}

/// Rebuild pagerank for every distinct user in the database.
/// Used by the admin endpoint when no user_id is specified.
pub async fn rebuild_all_users(db: &Database) -> Result<usize> {
    let mut rows = db
        .connection()
        .query(
            "SELECT DISTINCT user_id FROM memories WHERE is_forgotten = 0",
            (),
        )
        .await?;
    let mut user_ids: Vec<i64> = Vec::new();
    while let Some(row) = rows.next().await? {
        user_ids.push(row.get(0)?);
    }
    for &uid in &user_ids {
        let scores = compute_pagerank_for_user(db, uid).await?;
        persist_pagerank(db, uid, &scores).await?;
    }
    Ok(user_ids.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::memory;
    use crate::memory::search::hybrid_search;
    use crate::memory::types::{QuestionType, SearchRequest, StoreRequest};
    use std::time::{Duration, Instant};

    fn store_request(content: &str, user_id: i64) -> StoreRequest {
        StoreRequest {
            content: content.to_string(),
            category: "test".to_string(),
            source: "test".to_string(),
            importance: 5,
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        }
    }

    fn search_request(query: &str, user_id: i64, limit: usize) -> SearchRequest {
        SearchRequest {
            query: query.to_string(),
            embedding: None,
            limit: Some(limit),
            category: None,
            source: None,
            tags: None,
            threshold: None,
            user_id: Some(user_id),
            space_id: None,
            include_forgotten: None,
            mode: None,
            question_type: Some(QuestionType::FactRecall),
            expand_relationships: false,
            include_links: false,
            latest_only: true,
            source_filter: None,
        }
    }

    async fn dirty_state(db: &Database, user_id: i64) -> (i64, i64) {
        let mut rows = db
            .connection()
            .query(
                "SELECT dirty_count, last_refresh FROM pagerank_dirty WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await
            .expect("query pagerank_dirty");
        let row = rows
            .next()
            .await
            .expect("read pagerank_dirty row")
            .expect("pagerank_dirty row exists");
        (
            row.get(0).expect("dirty_count"),
            row.get(1).expect("last_refresh"),
        )
    }

    async fn pagerank_count(db: &Database, user_id: i64) -> i64 {
        let mut rows = db
            .connection()
            .query(
                "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
                libsql::params![user_id],
            )
            .await
            .expect("query memory_pagerank count");
        rows.next()
            .await
            .expect("read count row")
            .expect("count row exists")
            .get(0)
            .expect("count value")
    }

    async fn pagerank_row(db: &Database, memory_id: i64) -> (f64, i64) {
        let mut rows = db
            .connection()
            .query(
                "SELECT score, computed_at FROM memory_pagerank WHERE memory_id = ?1",
                libsql::params![memory_id],
            )
            .await
            .expect("query pagerank row");
        let row = rows
            .next()
            .await
            .expect("read pagerank row")
            .expect("pagerank row exists");
        (row.get(0).expect("score"), row.get(1).expect("computed_at"))
    }

    #[test]
    fn test_edge_weight() {
        assert!((edge_weight("caused_by", 0.5) - 1.0).abs() < 1e-10);
        assert!((edge_weight("related", 0.8) - 0.8).abs() < 1e-10);
        assert!((edge_weight("consolidates", 1.0) - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_pagerank_in_memory() {
        let mut pr: HashMap<i64, f64> = HashMap::new();
        pr.insert(1, 1.0 / 3.0);
        pr.insert(2, 1.0 / 3.0);
        pr.insert(3, 1.0 / 3.0);
        let mut out_w: HashMap<i64, f64> = HashMap::new();
        out_w.insert(1, 1.0);
        out_w.insert(2, 1.0);
        out_w.insert(3, 0.0);
        let mut in_links: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();
        in_links.insert(1, vec![]);
        in_links.insert(2, vec![(1, 1.0)]);
        in_links.insert(3, vec![(2, 1.0)]);
        let n = 3.0_f64;
        let damping = 0.85;
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

    #[tokio::test]
    async fn dirty_counter_increments_on_store_and_delete() {
        let db = Database::connect_memory().await.expect("in-memory db");
        let user_id = 1;

        let stored = memory::store(&db, store_request("dirty counter alpha 001", user_id))
            .await
            .expect("store memory");
        assert_eq!(dirty_state(&db, user_id).await, (1, 0));

        memory::delete(&db, stored.id, user_id)
            .await
            .expect("delete memory");
        assert_eq!(dirty_state(&db, user_id).await, (2, 0));
    }

    #[tokio::test]
    async fn persist_pagerank_upserts_and_zeroes_dirty_counter() {
        let db = Database::connect_memory().await.expect("in-memory db");
        let user_id = 1;

        let stored = memory::store(&db, store_request("persist pagerank alpha 002", user_id))
            .await
            .expect("store memory");
        assert_eq!(dirty_state(&db, user_id).await, (1, 0));

        persist_pagerank(&db, user_id, &[(stored.id, 0.25)])
            .await
            .expect("persist initial pagerank");
        let (score_one, computed_one) = pagerank_row(&db, stored.id).await;
        let (dirty_count_one, last_refresh_one) = dirty_state(&db, user_id).await;
        assert!((score_one - 0.25).abs() < 1e-10);
        assert_eq!(dirty_count_one, 0);
        assert!(last_refresh_one > 0);
        assert_eq!(computed_one, last_refresh_one);

        mark_pagerank_dirty(&db, user_id, 3)
            .await
            .expect("mark dirty again");
        assert_eq!(dirty_state(&db, user_id).await.0, 3);

        tokio::time::sleep(Duration::from_secs(1)).await;
        persist_pagerank(&db, user_id, &[(stored.id, 0.75)])
            .await
            .expect("persist updated pagerank");

        let (score_two, computed_two) = pagerank_row(&db, stored.id).await;
        let (dirty_count_two, last_refresh_two) = dirty_state(&db, user_id).await;
        assert!((score_two - 0.75).abs() < 1e-10);
        assert_eq!(dirty_count_two, 0);
        assert!(computed_two >= computed_one);
        assert!(last_refresh_two >= last_refresh_one);
    }

    #[tokio::test]
    async fn first_query_populates_cache_and_prefers_high_rank_memory() {
        let db = Database::connect_memory().await.expect("in-memory db");
        let user_id = 1;

        let center = memory::store(
            &db,
            store_request("alpha common hub signal center", user_id),
        )
        .await
        .expect("store center memory");
        let left = memory::store(&db, store_request("alpha common leaf signal left", user_id))
            .await
            .expect("store left memory");
        let right = memory::store(
            &db,
            store_request("alpha common leaf signal right", user_id),
        )
        .await
        .expect("store right memory");

        memory::insert_link(&db, left.id, center.id, 1.0, "causes", user_id)
            .await
            .expect("link left to center");
        memory::insert_link(&db, right.id, center.id, 1.0, "causes", user_id)
            .await
            .expect("link right to center");

        assert_eq!(pagerank_count(&db, user_id).await, 0);

        let results = hybrid_search(&db, search_request("alpha common signal", user_id, 3))
            .await
            .expect("hybrid search succeeds");

        assert_eq!(pagerank_count(&db, user_id).await, 3);
        assert!(results.len() >= 3);
        assert_eq!(results[0].memory.id, center.id);
    }

    #[tokio::test]
    async fn cached_search_returns_under_100ms_after_warm() {
        let db = Database::connect_memory().await.expect("in-memory db");
        let user_id = 1;
        let mut created = 0_i64;

        for i in 0..100 {
            let content = format!(
                "warm cache token node_{i} axis_{} shard_{} pulse_{}",
                i * 17,
                i * 31,
                i * 43
            );
            let stored = memory::store(&db, store_request(&content, user_id))
                .await
                .expect("store memory for warm search");
            if stored.created {
                created += 1;
            }
        }

        let warmup = hybrid_search(&db, search_request("warm cache token", user_id, 10))
            .await
            .expect("warmup search succeeds");
        assert!(!warmup.is_empty());
        assert_eq!(pagerank_count(&db, user_id).await, created);

        let started = Instant::now();
        let results = hybrid_search(&db, search_request("warm cache token", user_id, 10))
            .await
            .expect("cached search succeeds");
        let elapsed = started.elapsed();

        assert!(!results.is_empty());
        assert!(
            elapsed < Duration::from_millis(100),
            "cached search took {:?}, expected under 100ms",
            elapsed
        );
    }
}
