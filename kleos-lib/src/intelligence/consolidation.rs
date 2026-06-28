//! Consolidation -- auto-summarize memory clusters by merging similar memories.

use super::types::{ConsolidationRecord, SweepResult};
use crate::db::Database;
use crate::memory::types::Memory;
use crate::{EngError, Result};
use tracing::info;

/// Consolidate a set of similar memories into a single merged memory.
///
/// Merges content from the candidate memories, computes new importance
/// (max of the group), creates a consolidated memory, links the sources,
/// and records the consolidation.
#[tracing::instrument(skip(db, memory_ids), fields(memory_count = memory_ids.len(), user_id))]
pub async fn consolidate(db: &Database, memory_ids: &[String], user_id: i64) -> Result<Memory> {
    if memory_ids.is_empty() {
        return Err(EngError::InvalidInput(
            "memory_ids must not be empty".to_string(),
        ));
    }
    if memory_ids.len() > MAX_CLUSTER_SIZE {
        return Err(EngError::InvalidInput(format!(
            "refusing to consolidate {} memories (max {})",
            memory_ids.len(),
            MAX_CLUSTER_SIZE
        )));
    }

    // Parse all IDs upfront before any async work.
    let ids: Vec<i64> = memory_ids
        .iter()
        .map(|s| {
            s.parse::<i64>()
                .map_err(|_| EngError::InvalidInput(format!("invalid memory id: {}", s)))
        })
        .collect::<Result<Vec<_>>>()?;

    // Fetch all source memories in one read -- MUST belong to caller. The
    // `user_id = ?1` predicate makes that ownership guarantee real in single-DB
    // (shared) mode; in a single-owner shard it is a no-op.
    let ids_for_read = ids.clone();
    let sources: Vec<(i64, String, String, i32)> = db
        .read(move |conn| {
            let placeholders = ids_for_read
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT id, content, category, importance \
                 FROM memories WHERE id IN ({}) AND user_id = ?1 AND is_forgotten = 0",
                placeholders
            );
            let mut stmt = conn.prepare(&sql)?;
            let mut rows = stmt.query(rusqlite::params![user_id])?;
            let mut result = Vec::new();
            while let Some(row) = rows.next()? {
                result.push((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                ));
            }
            Ok(result)
        })
        .await?;

    if sources.is_empty() {
        return Err(EngError::NotFound(
            "no valid memories found for consolidation".to_string(),
        ));
    }

    // Reject if any requested ID was not found (caller doesn't own it).
    if sources.len() != memory_ids.len() {
        return Err(EngError::NotFound(
            "one or more memories not found or not owned".to_string(),
        ));
    }

    // Build merged content.
    let max_importance = sources.iter().map(|s| s.3).max().unwrap_or(5);
    let category = sources[0].2.clone();

    // Create a title from the first few words of the first memory (sources[0].1 = content).
    let title_words: Vec<&str> = sources[0].1.split_whitespace().take(5).collect();
    let title = title_words.join(" ");

    let merged_content = format!(
        "[Consolidated: {}]\n{}",
        title,
        sources
            .iter()
            .map(|s| format!("- {}", s.1))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let source_ids_json =
        serde_json::to_string(&sources.iter().map(|s| s.0).collect::<Vec<_>>()).unwrap_or_default();
    let source_count = sources.len() as i64;
    let sources_for_write: Vec<i64> = sources.iter().map(|s| s.0).collect();

    // All writes in a single transaction for atomicity.
    let new_id: i64 = db
        .transaction(move |tx| {
            // Insert consolidated memory, owned by the caller so single-DB mode
            // isolates it like any other memory.
            tx.execute(
                "INSERT INTO memories (content, category, source, importance, version, is_latest, \
                 source_count, is_static, is_forgotten, confidence, status, user_id, created_at, updated_at) \
                 VALUES (?1, ?2, 'consolidation', ?3, 1, 1, ?4, 1, 0, 1.0, 'approved', ?5, datetime('now'), datetime('now'))",
                rusqlite::params![
                    merged_content,
                    category,
                    max_importance,
                    source_count,
                    user_id,
                ],
            )
            ?;

            let new_id = tx.last_insert_rowid();

            // Link source memories to the consolidated memory and mark them consolidated.
            for source_id in &sources_for_write {
                tx.execute(
                    "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
                     VALUES (?1, ?2, 1.0, 'consolidates')",
                    rusqlite::params![new_id, source_id],
                )
                ?;
                tx.execute(
                    "UPDATE memories \
                     SET is_consolidated = 1, is_latest = 0, updated_at = datetime('now') \
                     WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![source_id, user_id],
                )
                ?;
            }

            // Record consolidation, owned by the caller.
            tx.execute(
                "INSERT INTO consolidations (source_ids, result_memory_id, strategy, confidence, user_id) \
                 VALUES (?1, ?2, 'merge', 1.0, ?3)",
                rusqlite::params![source_ids_json, new_id, user_id],
            )
            ?;

            Ok(new_id)
        })
        .await?;

    info!(
        summary_id = new_id,
        sources = source_count,
        user_id,
        "consolidated"
    );

    // Fetch and return the new memory.
    let memory = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, source, session_id, importance, version, \
                     is_latest, parent_memory_id, root_memory_id, source_count, is_static, \
                     is_forgotten, is_archived, is_fact, is_decomposed, \
                     forget_after, forget_reason, model, recall_hits, recall_misses, \
                     adaptive_score, pagerank_score, last_accessed_at, access_count, tags, \
                     episode_id, decay_score, confidence, sync_id, status, user_id, space_id, \
                     fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, \
                     fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, \
                     valence, arousal, dominant_emotion, created_at, updated_at, is_superseded, is_consolidated \
                     FROM memories WHERE id = ?1",
                )
                ?;
            let mut rows = stmt
                .query(rusqlite::params![new_id])
                ?;
            if let Some(row) = rows.next()? {
                row_to_memory(row)
            } else {
                Err(EngError::Internal(
                    "consolidated memory not found after insert".to_string(),
                ))
            }
        })
        .await?;

    Ok(memory)
}

/// Find groups of memories that are candidates for consolidation.
///
/// Uses memory_links similarity scores to find clusters of memories
/// with similarity above the threshold.
#[tracing::instrument(skip(db), fields(threshold, user_id))]
pub async fn find_consolidation_candidates(
    db: &Database,
    threshold: f32,
    user_id: i64,
) -> Result<Vec<Vec<String>>> {
    // Collect all similar pairs from the database.
    let pairs: Vec<(i64, i64)> = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT ml.source_id, ml.target_id \
                     FROM memory_links ml \
                     JOIN memories ms ON ms.id = ml.source_id \
                     JOIN memories mt ON mt.id = ml.target_id \
                     WHERE ml.similarity >= ?1 \
                       AND ml.type IN ('similarity', 'related', 'cite') \
                       AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
                       AND ms.is_latest = 1 AND mt.is_latest = 1 \
                       AND ms.is_archived = 0 AND mt.is_archived = 0 \
                       AND ms.is_consolidated = 0 AND mt.is_consolidated = 0 \
                       AND ms.user_id = ?2 AND mt.user_id = ?2 \
                     ORDER BY ml.similarity DESC \
                     LIMIT 200",
            )?;
            let mut rows = stmt.query(rusqlite::params![threshold as f64, user_id])?;
            let mut pairs = Vec::new();
            while let Some(row) = rows.next()? {
                let source_id: i64 = row.get(0)?;
                let target_id: i64 = row.get(1)?;
                pairs.push((source_id, target_id));
            }
            Ok(pairs)
        })
        .await?;

    // Simple union-find to cluster connected pairs.
    let mut parent: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();

    /// Find the representative root for a memory id in the clustering set.
    fn find(parent: &mut std::collections::HashMap<i64, i64>, x: i64) -> i64 {
        if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(x) {
            e.insert(x);
            return x;
        }
        let mut root = x;
        while parent[&root] != root {
            root = parent[&root];
        }
        // Path compression
        let mut current = x;
        while current != root {
            let next = parent[&current];
            parent.insert(current, root);
            current = next;
        }
        root
    }

    /// Merge two memory ids into the same clustering set.
    fn union(parent: &mut std::collections::HashMap<i64, i64>, a: i64, b: i64) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    for (source_id, target_id) in pairs {
        union(&mut parent, source_id, target_id);
    }

    // Group by root.
    let mut clusters: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    let keys: Vec<i64> = parent.keys().copied().collect();
    for id in keys {
        let root = find(&mut parent, id);
        clusters.entry(root).or_default().push(id.to_string());
    }

    // Only return clusters with 2+ members.
    let result: Vec<Vec<String>> = clusters.into_values().filter(|c| c.len() >= 2).collect();

    Ok(result)
}

/// Maximum number of memories allowed in a single consolidation group.
/// Larger clusters produce unreadable merged content and destroy source recall.
const MAX_CLUSTER_SIZE: usize = 5;

/// Maximum consolidations performed per sweep run to limit blast radius.
const MAX_CONSOLIDATIONS_PER_SWEEP: i64 = 10;

/// Minimum acceptable similarity threshold -- rejects sweeps that would merge
/// loosely related memories.
const MIN_SIMILARITY_THRESHOLD: f64 = 0.80;

/// Run an automatic consolidation sweep: find candidate groups above the
/// given similarity threshold and consolidate each group.
///
/// Safety guardrails:
/// - Rejects thresholds below 0.80 to prevent loose matches
/// - Skips clusters larger than 5 memories
/// - Stops after 10 consolidations per sweep
#[tracing::instrument(skip(db))]
pub async fn sweep(db: &Database, user_id: i64, threshold: f64) -> Result<SweepResult> {
    if threshold < MIN_SIMILARITY_THRESHOLD {
        return Err(EngError::InvalidInput(format!(
            "similarity threshold {threshold} is below minimum {MIN_SIMILARITY_THRESHOLD}"
        )));
    }

    let groups = find_consolidation_candidates(db, threshold as f32, user_id).await?;
    let pairs_found = groups.len() as i64;
    let mut consolidated = 0i64;
    let mut skipped = 0i64;

    for group in &groups {
        if consolidated >= MAX_CONSOLIDATIONS_PER_SWEEP {
            skipped += 1;
            continue;
        }
        if group.len() < 2 {
            continue;
        }
        if group.len() > MAX_CLUSTER_SIZE {
            tracing::warn!(
                cluster_size = group.len(),
                user_id,
                "skipping oversized consolidation cluster"
            );
            skipped += 1;
            continue;
        }
        match consolidate(db, group, user_id).await {
            Ok(_) => consolidated += 1,
            Err(e) => {
                tracing::warn!(error = %e, "sweep_consolidation_failed");
                skipped += 1;
            }
        }
    }

    Ok(SweepResult {
        pairs_found,
        consolidated,
        skipped,
    })
}

/// List recent consolidation records.
#[tracing::instrument(skip(db), fields(limit))]
pub async fn list_consolidations(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<ConsolidationRecord>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT c.id, m.content \
                 FROM consolidations c \
                 JOIN memories m ON m.id = c.result_memory_id \
                 WHERE c.user_id = ?1 \
                 ORDER BY c.created_at DESC \
                 LIMIT ?2",
        )?;
        let mut rows = stmt.query(rusqlite::params![user_id, limit as i64])?;
        let mut records = Vec::new();
        while let Some(row) = rows.next()? {
            records.push(ConsolidationRecord {
                id: row.get(0)?,
                summary: row.get(1)?,
            });
        }
        Ok(records)
    })
    .await
}

/// Convert a consolidation SELECT row into a Memory value.
fn row_to_memory(row: &rusqlite::Row<'_>) -> crate::Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        content: row.get(1)?,
        category: row.get(2)?,
        source: row.get(3)?,
        session_id: row.get(4)?,
        importance: row.get(5)?,
        embedding: None,
        version: row.get(6)?,
        is_latest: row.get::<_, i32>(7)? != 0,
        parent_memory_id: row.get(8)?,
        root_memory_id: row.get(9)?,
        source_count: row.get(10)?,
        is_static: row.get::<_, i32>(11)? != 0,
        is_forgotten: row.get::<_, i32>(12)? != 0,
        is_archived: row.get::<_, i32>(13)? != 0,
        is_fact: row.get::<_, i32>(14)? != 0,
        is_decomposed: row.get::<_, i32>(15)? != 0,
        forget_after: row.get(16)?,
        forget_reason: row.get(17)?,
        model: row.get(18)?,
        recall_hits: row.get(19)?,
        recall_misses: row.get(20)?,
        adaptive_score: row.get(21)?,
        pagerank_score: row.get(22)?,
        last_accessed_at: row.get(23)?,
        access_count: row.get(24)?,
        tags: row.get(25)?,
        episode_id: row.get(26)?,
        decay_score: row.get(27)?,
        confidence: row.get(28)?,
        sync_id: row.get(29)?,
        status: row.get(30)?,
        user_id: row.get(31)?,
        space_id: row.get(32)?,
        fsrs_stability: row.get(33)?,
        fsrs_difficulty: row.get(34)?,
        fsrs_storage_strength: row.get(35)?,
        fsrs_retrieval_strength: row.get(36)?,
        fsrs_learning_state: row.get(37)?,
        fsrs_reps: row.get(38)?,
        fsrs_lapses: row.get(39)?,
        fsrs_last_review_at: row.get(40)?,
        valence: row.get(41)?,
        arousal: row.get(42)?,
        dominant_emotion: row.get(43)?,
        created_at: row.get(44)?,
        updated_at: row.get(45)?,
        is_superseded: row.get::<_, i32>(46)? != 0,
        is_consolidated: row.get::<_, i32>(47)? != 0,
        // This custom SELECT does not fetch lang; not used by consolidation logic.
        lang: None,
    })
}

/// Unit tests for consolidation helper algorithms.
#[cfg(test)]
mod tests {
    /// Verifies union-find clustering merges connected components.
    #[test]
    fn test_union_find_clustering() {
        let mut parent = std::collections::HashMap::new();

        /// Find the representative root for a test id.
        fn find(parent: &mut std::collections::HashMap<i64, i64>, x: i64) -> i64 {
            if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(x) {
                e.insert(x);
                return x;
            }
            let mut root = x;
            while parent[&root] != root {
                root = parent[&root];
            }
            root
        }

        /// Merge two test ids into one component.
        fn union(parent: &mut std::collections::HashMap<i64, i64>, a: i64, b: i64) {
            let ra = find(parent, a);
            let rb = find(parent, b);
            if ra != rb {
                parent.insert(ra, rb);
            }
        }

        // Create clusters: {1,2,3} and {4,5}
        union(&mut parent, 1, 2);
        union(&mut parent, 2, 3);
        union(&mut parent, 4, 5);

        assert_eq!(find(&mut parent, 1), find(&mut parent, 3));
        assert_ne!(find(&mut parent, 1), find(&mut parent, 4));
        assert_eq!(find(&mut parent, 4), find(&mut parent, 5));
    }
}
