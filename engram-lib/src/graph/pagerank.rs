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

    let mut edge_rows = conn
        .query(
            "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
         FROM memory_links ml \
         JOIN memories ms ON ms.id = ml.source_id \
         JOIN memories mt ON mt.id = ml.target_id \
         WHERE ms.user_id = ?1 AND mt.user_id = ?1 \
           AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
           AND ms.is_archived = 0 AND mt.is_archived = 0",
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
    let max_rank = result
        .scores
        .values()
        .copied()
        .fold(0.0_f64, f64::max);
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
        .query("SELECT DISTINCT user_id FROM memories WHERE is_forgotten = 0", ())
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
        (
            row.get(0).expect("score"),
            row.get(1).expect("computed_at"),
        )
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

        let center = memory::store(&db, store_request("alpha common hub signal center", user_id))
            .await
            .expect("store center memory");
        let left = memory::store(&db, store_request("alpha common leaf signal left", user_id))
            .await
            .expect("store left memory");
        let right = memory::store(&db, store_request("alpha common leaf signal right", user_id))
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
