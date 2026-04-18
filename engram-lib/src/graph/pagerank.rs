// ============================================================================
// PAGERANK -- iterative weighted PageRank for memory graph
// ============================================================================

use super::types::{PageRankResult, PageRankUpdateResult};
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use tracing::info;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

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

#[tracing::instrument(skip(db))]
pub async fn compute_pagerank(
    db: &Database,
    user_id: i64,
    damping: f64,
    max_iterations: u32,
) -> Result<PageRankResult> {
    let memory_ids: Vec<i64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let n = memory_ids.len();
    if n == 0 {
        return Ok(PageRankResult {
            scores: HashMap::new(),
            iterations: 0,
        });
    }

    // GROUP BY deduplicates multiple memory_links rows for the same
    // (source_id, target_id) pair. Duplicates would inflate out_w and
    // in_links, distorting PageRank scores (RB-L8).
    let edges: Vec<(i64, i64, f64, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT ml.source_id, ml.target_id, MAX(ml.similarity), MAX(ml.type) \
                     FROM memory_links ml \
                     JOIN memories ms ON ms.id = ml.source_id \
                     JOIN memories mt ON mt.id = ml.target_id \
                     WHERE ms.user_id = ?1 AND mt.user_id = ?1 \
                       AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
                       AND ms.is_archived = 0 AND mt.is_archived = 0 \
                     GROUP BY ml.source_id, ml.target_id",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let mut pr: HashMap<i64, f64> = HashMap::new();
    let mut out_w: HashMap<i64, f64> = HashMap::new();
    let mut in_links: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();
    let mem_set: std::collections::HashSet<i64> = memory_ids.iter().copied().collect();

    for &id in &memory_ids {
        pr.insert(id, 1.0 / n as f64);
        out_w.insert(id, 0.0);
        in_links.insert(id, Vec::new());
    }

    for (source_id, target_id, similarity, link_type) in edges {
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

#[tracing::instrument(skip(db))]
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

    let scores_vec: Vec<(i64, f64)> = {
        // Temporal decay: reduce PageRank for older memories so stale nodes don't
        // dominate graph traversal. Uses a true half-life: score * 0.5^(age/half_life).
        let half_life: f64 = std::env::var("ENGRAM_PAGERANK_HALF_LIFE_DAYS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(180.0);

        // Fetch memory ages in one query (julianday diff).
        let uid = user_id;
        let ages: HashMap<i64, f64> = db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, julianday('now') - julianday(created_at) \
                         FROM memories WHERE user_id = ?1",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![uid])
                    .map_err(rusqlite_to_eng_error)?;
                let mut m = HashMap::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    let id: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
                    let age_days: f64 = row.get(1).unwrap_or(0.0);
                    m.insert(id, age_days);
                }
                Ok(m)
            })
            .await?;

        // ln(2) for true half-life decay.
        const LN2: f64 = std::f64::consts::LN_2;
        let decay_floor = 0.05;

        result
            .scores
            .iter()
            .map(|(&id, &rank)| {
                let normalized = rank / max_rank;
                let age_days = ages.get(&id).copied().unwrap_or(0.0).max(0.0);
                let decay = (-(age_days * LN2 / half_life)).exp().max(decay_floor);
                (id, normalized * decay)
            })
            .collect()
    };
    let memories_count = scores_vec.len();

    // Wrap batch UPDATEs in transaction for atomicity (S1-5/S1-6 fix).
    // Use prepare_cached so the statement is parsed once and reused across N rows.
    db.transaction(move |tx| {
        let mut stmt = tx
            .prepare_cached(
                "UPDATE memories SET pagerank_score = ?1 WHERE id = ?2 AND user_id = ?3",
            )
            .map_err(rusqlite_to_eng_error)?;
        for (id, normalized) in &scores_vec {
            stmt.execute(rusqlite::params![normalized, id, user_id])
                .map_err(rusqlite_to_eng_error)?;
        }
        Ok(())
    })
    .await?;

    info!(
        user_id,
        memories = memories_count,
        iterations = result.iterations,
        max_raw = format!("{:.6}", max_rank).as_str(),
        "pagerank_updated"
    );
    Ok(PageRankUpdateResult {
        memories: memories_count,
        iterations: result.iterations,
    })
}

/// Compute normalized PageRank scores for a user and return as a vec of (memory_id, score).
/// Does not write to any table.
#[tracing::instrument(skip(db))]
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
#[tracing::instrument(skip(db))]
pub async fn snapshot_pagerank_dirty(db: &Database, user_id: i64) -> Result<i64> {
    db.read(move |conn| {
        let result = conn
            .query_row(
                "SELECT dirty_count FROM pagerank_dirty WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?;
        Ok(result.unwrap_or(0))
    })
    .await
}

/// Upsert computed scores into `memory_pagerank` and subtract the supplied
/// dirty snapshot from the counter (clamped at zero). See
/// [`snapshot_pagerank_dirty`] for the usage pattern that avoids losing
/// concurrent writes.
#[tracing::instrument(skip(db, scores), fields(score_count = scores.len()))]
pub async fn persist_pagerank_with_snapshot(
    db: &Database,
    user_id: i64,
    scores: &[(i64, f64)],
    dirty_snapshot: i64,
) -> Result<()> {
    let scores_owned: Vec<(i64, f64)> = scores.to_vec();
    let now = chrono::Utc::now().timestamp();

    db.transaction(move |tx| {
        for &(memory_id, score) in &scores_owned {
            tx.execute(
                "INSERT INTO memory_pagerank (memory_id, user_id, score, computed_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(memory_id) DO UPDATE SET \
                   score = excluded.score, \
                   computed_at = excluded.computed_at",
                rusqlite::params![memory_id, user_id, score, now],
            )
            .map_err(rusqlite_to_eng_error)?;
        }
        // Subtract only the increments we compensated for. Any concurrent
        // mark_pagerank_dirty that fired while compute was running remains in
        // the counter and schedules the next refresh cycle. Clamped at 0 so a
        // spurious over-count cannot push the value negative.
        tx.execute(
            "INSERT INTO pagerank_dirty (user_id, dirty_count, last_refresh) \
             VALUES (?1, 0, ?2) \
             ON CONFLICT(user_id) DO UPDATE SET \
               dirty_count = MAX(0, dirty_count - ?3), \
               last_refresh = excluded.last_refresh",
            rusqlite::params![user_id, now, dirty_snapshot],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
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
#[tracing::instrument(skip(db, scores), fields(score_count = scores.len()))]
pub async fn persist_pagerank(db: &Database, user_id: i64, scores: &[(i64, f64)]) -> Result<()> {
    let snapshot = snapshot_pagerank_dirty(db, user_id).await?;
    persist_pagerank_with_snapshot(db, user_id, scores, snapshot).await
}

/// Increment the dirty counter for a user. Called after memory/edge mutations.
#[tracing::instrument(skip(db))]
pub async fn mark_pagerank_dirty(db: &Database, user_id: i64, delta: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO pagerank_dirty (user_id, dirty_count, last_refresh) \
             VALUES (?1, ?2, 0) \
             ON CONFLICT(user_id) DO UPDATE SET dirty_count = dirty_count + ?2",
            rusqlite::params![user_id, delta],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

// ============================================================================
// INCREMENTAL PAGERANK -- delta updates without full recompute
// ============================================================================

const DAMPING: f64 = 0.85;
const CONVERGENCE_THRESHOLD: f64 = 1e-6;

/// Incremental PageRank update when a new memory is added.
/// Inserts the memory with base rank and does NOT trigger full recompute.
/// The new node has no incoming links yet, so it gets the teleportation score only.
#[tracing::instrument(skip(db))]
pub async fn incremental_add_memory(db: &Database, memory_id: i64, user_id: i64) -> Result<()> {
    // Get current memory count to compute base rank
    let n: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memories \
                 WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    // Base rank for new node: (1-d)/N
    let base_rank = (1.0 - DAMPING) / n.max(1) as f64;
    let now = chrono::Utc::now().timestamp();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO memory_pagerank (memory_id, user_id, score, computed_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(memory_id) DO UPDATE SET \
               score = excluded.score, computed_at = excluded.computed_at",
            rusqlite::params![memory_id, user_id, base_rank, now],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Incremental PageRank update when a link is added.
/// Propagates score changes locally to affected nodes (2-hop neighborhood).
#[tracing::instrument(skip(db))]
pub async fn incremental_add_link(
    db: &Database,
    source_id: i64,
    target_id: i64,
    similarity: f64,
    link_type: &str,
    user_id: i64,
) -> Result<usize> {
    let link_type = link_type.to_string();

    // Get current scores for source and target
    let scores: HashMap<i64, f64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT memory_id, score FROM memory_pagerank \
                     WHERE memory_id IN (?1, ?2) AND user_id = ?3",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![source_id, target_id, user_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
                })
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<HashMap<i64, f64>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    // If neither node has a score, initialize them
    if scores.is_empty() {
        incremental_add_memory(db, source_id, user_id).await?;
        incremental_add_memory(db, target_id, user_id).await?;
        return Ok(2);
    }

    let source_score = scores.get(&source_id).copied().unwrap_or(0.01);
    let weight = edge_weight(&link_type, similarity);

    // Get source's total outgoing weight
    let total_out: f64 = db
        .read(move |conn| {
            let result = conn
                .query_row(
                    "SELECT SUM(similarity) FROM memory_links WHERE source_id = ?1",
                    rusqlite::params![source_id],
                    |row| row.get::<_, Option<f64>>(0),
                )
                .optional()
                .map_err(rusqlite_to_eng_error)?;
            Ok(result.flatten().unwrap_or(1.0))
        })
        .await?;

    // Compute contribution from source to target
    let contribution = DAMPING * source_score * weight / total_out.max(weight);

    // Update target score
    let old_target = scores.get(&target_id).copied().unwrap_or(0.01);
    let new_target = old_target + contribution;
    let now = chrono::Utc::now().timestamp();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO memory_pagerank (memory_id, user_id, score, computed_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(memory_id) DO UPDATE SET \
               score = excluded.score, computed_at = excluded.computed_at",
            rusqlite::params![target_id, user_id, new_target, now],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    // Propagate to target's neighbors (1-hop)
    let neighbors: Vec<(i64, f64, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT target_id, similarity, type FROM memory_links \
                     WHERE source_id = ?1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![target_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, f64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let mut updated = 1usize;
    let delta = new_target - old_target;

    for (neighbor_id, sim, lt) in neighbors {
        let w = edge_weight(&lt, sim);
        let neighbor_contribution = DAMPING * delta * w / total_out.max(1.0);

        if neighbor_contribution.abs() > CONVERGENCE_THRESHOLD {
            db.write(move |conn| {
                conn.execute(
                    "UPDATE memory_pagerank SET score = score + ?1, computed_at = ?2 \
                     WHERE memory_id = ?3 AND user_id = ?4",
                    rusqlite::params![neighbor_contribution, now, neighbor_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
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
#[tracing::instrument(skip(db))]
pub async fn incremental_remove_memory(db: &Database, memory_id: i64, user_id: i64) -> Result<()> {
    // Get the score being removed
    let removed_score: Option<f64> = db
        .read(move |conn| {
            conn.query_row(
                "SELECT score FROM memory_pagerank \
                 WHERE memory_id = ?1 AND user_id = ?2",
                rusqlite::params![memory_id, user_id],
                |row| row.get::<_, f64>(0),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let removed_score = match removed_score {
        Some(s) => s,
        None => return Ok(()), // No score to remove
    };

    // Delete the score
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM memory_pagerank WHERE memory_id = ?1 AND user_id = ?2",
            rusqlite::params![memory_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    // Get remaining memory count
    let remaining: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    if remaining > 0 {
        // Distribute removed score evenly (simplified redistribution)
        let redistribution = removed_score / remaining as f64;
        let now = chrono::Utc::now().timestamp();

        db.write(move |conn| {
            conn.execute(
                "UPDATE memory_pagerank SET score = score + ?1, computed_at = ?2 \
                 WHERE user_id = ?3",
                rusqlite::params![redistribution, now, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await?;
    }

    info!(
        memory_id,
        removed_score = format!("{:.6}", removed_score).as_str(),
        "incremental_pagerank_remove"
    );

    Ok(())
}

/// Check if incremental updates have drifted too far from true PageRank.
/// Returns true if a full recompute is recommended.
#[tracing::instrument(skip(db))]
pub async fn needs_full_recompute(
    db: &Database,
    user_id: i64,
    drift_threshold: f64,
) -> Result<bool> {
    // Compare sum of incremental scores to expected sum (should be ~1.0)
    db.read(move |conn| {
        let result = conn
            .query_row(
                "SELECT SUM(score), COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(rusqlite_to_eng_error)?;
        match result {
            Some((sum_opt, count)) => {
                let sum: f64 = sum_opt.unwrap_or(0.0);
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
    })
    .await
}

// ============================================================================
// COMMUNITY-SCOPED PAGERANK -- compute per community for reduced memory usage
// ============================================================================

/// Compute PageRank for a single community only.
/// Much more memory-efficient than global compute for large graphs.
#[tracing::instrument(skip(db))]
pub async fn compute_pagerank_for_community(
    db: &Database,
    user_id: i64,
    community_id: i64,
    damping: f64,
    max_iterations: u32,
) -> Result<PageRankResult> {
    let memory_ids: Vec<i64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM memories \
                     WHERE user_id = ?1 AND community_id = ?2 \
                       AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id, community_id], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let n = memory_ids.len();
    if n == 0 {
        return Ok(PageRankResult {
            scores: HashMap::new(),
            iterations: 0,
        });
    }

    // Build ID set for fast lookup
    let mem_set: std::collections::HashSet<i64> = memory_ids.iter().copied().collect();

    // SECURITY (SEC-H6): use a temp table + JOIN instead of a dynamic IN(...)
    // clause. The old approach built an unbounded SQL string with one element
    // per memory_id, causing O(n^2) query cost for large communities.
    let ids_for_sql = memory_ids.clone();
    let edges: Vec<(i64, i64, f64, String)> = db
        .read(move |conn| {
            conn.execute_batch("CREATE TEMP TABLE IF NOT EXISTS _pr_ids (id INTEGER PRIMARY KEY)")
                .map_err(rusqlite_to_eng_error)?;
            conn.execute("DELETE FROM temp._pr_ids", [])
                .map_err(rusqlite_to_eng_error)?;
            {
                let mut ins = conn
                    .prepare("INSERT OR IGNORE INTO temp._pr_ids (id) VALUES (?1)")
                    .map_err(rusqlite_to_eng_error)?;
                for id in &ids_for_sql {
                    ins.execute(rusqlite::params![id])
                        .map_err(rusqlite_to_eng_error)?;
                }
            }
            let mut stmt = conn
                .prepare(
                    "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
                 FROM memory_links ml \
                 INNER JOIN temp._pr_ids s ON ml.source_id = s.id \
                 INNER JOIN temp._pr_ids t ON ml.target_id = t.id",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let mut pr: HashMap<i64, f64> = HashMap::new();
    let mut out_w: HashMap<i64, f64> = HashMap::new();
    let mut in_links: HashMap<i64, Vec<(i64, f64)>> = HashMap::new();

    for &id in &memory_ids {
        pr.insert(id, 1.0 / n as f64);
        out_w.insert(id, 0.0);
        in_links.insert(id, Vec::new());
    }

    for (source_id, target_id, similarity, link_type) in edges {
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
#[tracing::instrument(skip(db))]
pub async fn compute_pagerank_by_communities(
    db: &Database,
    user_id: i64,
) -> Result<Vec<(i64, f64)>> {
    // Get all distinct community IDs for this user
    let community_ids: Vec<i64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT community_id FROM memories \
                     WHERE user_id = ?1 AND community_id IS NOT NULL \
                       AND is_forgotten = 0 AND is_latest = 1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

    // Also handle memories without community (community_id IS NULL)
    let orphan_ids: Vec<i64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM memories \
                     WHERE user_id = ?1 AND community_id IS NULL \
                       AND is_forgotten = 0 AND is_latest = 1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

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
#[tracing::instrument(skip(db))]
pub async fn ensure_pagerank_for_user(db: &Database, user_id: i64) -> Result<()> {
    let count: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1 LIMIT 1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

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
#[tracing::instrument(skip(db))]
pub async fn rebuild_all_users(db: &Database) -> Result<usize> {
    let user_ids: Vec<i64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT DISTINCT user_id FROM memories WHERE is_forgotten = 0")
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map([], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()
                .map_err(rusqlite_to_eng_error)
        })
        .await?;

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
        db.read(move |conn| {
            conn.query_row(
                "SELECT dirty_count, last_refresh FROM pagerank_dirty WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await
        .expect("query pagerank_dirty")
    }

    async fn pagerank_count(db: &Database, user_id: i64) -> i64 {
        db.read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await
        .expect("query memory_pagerank count")
    }

    async fn pagerank_row(db: &Database, memory_id: i64) -> (f64, i64) {
        db.read(move |conn| {
            conn.query_row(
                "SELECT score, computed_at FROM memory_pagerank WHERE memory_id = ?1",
                rusqlite::params![memory_id],
                |row| Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await
        .expect("query pagerank row")
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
