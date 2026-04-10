//! Consolidation -- auto-summarize memory clusters by merging similar memories.

use crate::db::Database;
use crate::memory::types::Memory;
use crate::Result;
use serde::Serialize;
use tracing::info;

/// Consolidate a set of similar memories into a single merged memory.
///
/// Merges content from the candidate memories, computes new importance
/// (max of the group), creates a consolidated memory, links the sources,
/// and records the consolidation.
pub async fn consolidate(db: &Database, memory_ids: &[String], _user_id: i64) -> Result<Memory> {
    let conn = db.connection();

    if memory_ids.is_empty() {
        return Err(crate::EngError::InvalidInput(
            "memory_ids must not be empty".to_string(),
        ));
    }

    // Fetch all source memories
    let mut sources: Vec<(i64, String, String, i32, i64)> = Vec::new();

    for id_str in memory_ids {
        let id: i64 = id_str
            .parse()
            .map_err(|_| crate::EngError::InvalidInput(format!("invalid memory id: {}", id_str)))?;

        let mut rows = conn
            .query(
                "SELECT id, content, category, importance, user_id \
                 FROM memories WHERE id = ?1 AND is_forgotten = 0",
                libsql::params![id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            sources.push((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ));
        }
    }

    if sources.is_empty() {
        return Err(crate::EngError::NotFound(
            "no valid memories found for consolidation".to_string(),
        ));
    }

    // Build merged content
    let max_importance = sources.iter().map(|s| s.3).max().unwrap_or(5);
    let user_id = sources[0].4;
    let category = sources[0].2.clone();

    // Create a title from the first few words of the first memory
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

    // Insert consolidated memory
    conn.execute(
        "INSERT INTO memories (content, category, source, importance, version, is_latest, \
         source_count, is_static, is_forgotten, confidence, status, user_id, created_at, updated_at) \
         VALUES (?1, ?2, 'consolidation', ?3, 1, 1, ?4, 1, 0, 1.0, 'approved', ?5, datetime('now'), datetime('now'))",
        libsql::params![
            merged_content.clone(),
            category.clone(),
            max_importance,
            sources.len() as i64,
            user_id
        ],
    )
    .await?;

    // Get the ID of the newly inserted memory
    let mut id_row = conn
        .query("SELECT last_insert_rowid()", ())
        .await?;
    let new_id: i64 = if let Some(row) = id_row.next().await? {
        row.get(0)?
    } else {
        return Err(crate::EngError::Internal(
            "failed to get new memory id".to_string(),
        ));
    };

    // Link source memories to the consolidated memory
    for &(source_id, _, _, _, _) in &sources {
        conn.execute(
            "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) \
             VALUES (?1, ?2, 1.0, 'consolidates')",
            libsql::params![new_id, source_id],
        )
        .await?;
    }

    // Record consolidation
    let source_ids_json =
        serde_json::to_string(&sources.iter().map(|s| s.0).collect::<Vec<_>>())
            .unwrap_or_default();

    conn.execute(
        "INSERT INTO consolidations (source_ids, result_memory_id, strategy, confidence, user_id) \
         VALUES (?1, ?2, 'merge', 1.0, ?3)",
        libsql::params![source_ids_json, new_id, user_id],
    )
    .await?;

    info!(
        summary_id = new_id,
        sources = sources.len(),
        "consolidated"
    );

    // Fetch and return the new memory
    let mut result_rows = conn
        .query(
            "SELECT id, content, category, source, session_id, importance, version, \
             is_latest, parent_memory_id, root_memory_id, source_count, is_static, \
             is_forgotten, is_archived, is_inference, is_fact, is_decomposed, \
             forget_after, forget_reason, model, recall_hits, recall_misses, \
             adaptive_score, pagerank_score, last_accessed_at, access_count, tags, \
             episode_id, decay_score, confidence, sync_id, status, user_id, space_id, \
             fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, \
             fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, \
             valence, arousal, dominant_emotion, created_at, updated_at, is_superseded \
             FROM memories WHERE id = ?1",
            libsql::params![new_id],
        )
        .await?;

    if let Some(row) = result_rows.next().await? {
        Ok(row_to_memory(&row)?)
    } else {
        Err(crate::EngError::Internal(
            "consolidated memory not found after insert".to_string(),
        ))
    }
}

/// Find groups of memories that are candidates for consolidation.
///
/// Uses memory_links similarity scores to find clusters of memories
/// with similarity above the threshold.
pub async fn find_consolidation_candidates(
    db: &Database,
    threshold: f32,
    user_id: i64,
) -> Result<Vec<Vec<String>>> {
    let conn = db.connection();

    // Find pairs of memories with high similarity
    let mut rows = conn
        .query(
            "SELECT ml.source_id, ml.target_id, ml.similarity \
             FROM memory_links ml \
             JOIN memories ms ON ms.id = ml.source_id \
             JOIN memories mt ON mt.id = ml.target_id \
             WHERE ml.similarity >= ?1 \
               AND ms.user_id = ?2 AND mt.user_id = ?2 \
               AND ml.type IN ('similarity', 'related', 'cite') \
               AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
               AND ms.is_latest = 1 AND mt.is_latest = 1 \
               AND ms.is_archived = 0 AND mt.is_archived = 0 \
             ORDER BY ml.similarity DESC \
             LIMIT 200",
            libsql::params![threshold as f64, user_id],
        )
        .await?;

    // Simple union-find to cluster connected pairs
    let mut parent: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();

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

    fn union(parent: &mut std::collections::HashMap<i64, i64>, a: i64, b: i64) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent.insert(ra, rb);
        }
    }

    while let Some(row) = rows.next().await? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        union(&mut parent, source_id, target_id);
    }

    // Group by root
    let mut clusters: std::collections::HashMap<i64, Vec<String>> =
        std::collections::HashMap::new();
    let keys: Vec<i64> = parent.keys().copied().collect();
    for id in keys {
        let root = find(&mut parent, id);
        clusters
            .entry(root)
            .or_default()
            .push(id.to_string());
    }

    // Only return clusters with 2+ members
    let result: Vec<Vec<String>> = clusters
        .into_values()
        .filter(|c| c.len() >= 2)
        .collect();

    Ok(result)
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationRecord {
    pub id: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SweepResult {
    pub pairs_found: i64,
    pub consolidated: i64,
}

/// Run an automatic consolidation sweep: find candidate groups above the
/// given similarity threshold and consolidate each group.
pub async fn sweep(
    db: &Database,
    user_id: i64,
    threshold: f64,
) -> Result<SweepResult> {
    let groups = find_consolidation_candidates(db, threshold as f32, user_id).await?;
    let pairs_found = groups.len() as i64;
    let mut consolidated = 0i64;

    for group in &groups {
        if group.len() < 2 {
            continue;
        }
        match consolidate(db, group, user_id).await {
            Ok(_) => consolidated += 1,
            Err(e) => {
                tracing::warn!(error = %e, "sweep_consolidation_failed");
            }
        }
    }

    Ok(SweepResult {
        pairs_found,
        consolidated,
    })
}

/// List recent consolidation records.
pub async fn list_consolidations(
    db: &Database,
    user_id: i64,
    limit: usize,
) -> Result<Vec<ConsolidationRecord>> {
    let conn = db.connection();

    let mut rows = conn
        .query(
            "SELECT c.id, m.content \
             FROM consolidations c \
             JOIN memories m ON m.id = c.result_memory_id \
             WHERE c.user_id = ?1 \
             ORDER BY c.created_at DESC \
             LIMIT ?2",
            libsql::params![user_id, limit as i64],
        )
        .await?;

    let mut records = Vec::new();
    while let Some(row) = rows.next().await? {
        records.push(ConsolidationRecord {
            id: row.get(0)?,
            summary: row.get(1)?,
        });
    }

    Ok(records)
}

fn row_to_memory(row: &libsql::Row) -> crate::Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        content: row.get(1)?,
        category: row.get(2)?,
        source: row.get(3)?,
        session_id: row.get(4)?,
        importance: row.get(5)?,
        embedding: None, // Skip blob deserialization in graph context
        version: row.get(6)?,
        is_latest: row.get::<i64>(7).map(|v| v != 0)?,
        parent_memory_id: row.get(8)?,
        root_memory_id: row.get(9)?,
        source_count: row.get(10)?,
        is_static: row.get::<i64>(11).map(|v| v != 0)?,
        is_forgotten: row.get::<i64>(12).map(|v| v != 0)?,
        is_archived: row.get::<i64>(13).map(|v| v != 0)?,
        is_inference: row.get::<i64>(14).map(|v| v != 0)?,
        is_fact: row.get::<i64>(15).map(|v| v != 0)?,
        is_decomposed: row.get::<i64>(16).map(|v| v != 0)?,
        forget_after: row.get(17)?,
        forget_reason: row.get(18)?,
        model: row.get(19)?,
        recall_hits: row.get(20)?,
        recall_misses: row.get(21)?,
        adaptive_score: row.get(22)?,
        pagerank_score: row.get(23)?,
        last_accessed_at: row.get(24)?,
        access_count: row.get(25)?,
        tags: row.get(26)?,
        episode_id: row.get(27)?,
        decay_score: row.get(28)?,
        confidence: row.get(29)?,
        sync_id: row.get(30)?,
        status: row.get(31)?,
        user_id: row.get(32)?,
        space_id: row.get(33)?,
        fsrs_stability: row.get(34)?,
        fsrs_difficulty: row.get(35)?,
        fsrs_storage_strength: row.get(36)?,
        fsrs_retrieval_strength: row.get(37)?,
        fsrs_learning_state: row.get(38)?,
        fsrs_reps: row.get(39)?,
        fsrs_lapses: row.get(40)?,
        fsrs_last_review_at: row.get(41)?,
        valence: row.get(42)?,
        arousal: row.get(43)?,
        dominant_emotion: row.get(44)?,
        created_at: row.get(45)?,
        updated_at: row.get(46)?,
        is_superseded: row.get::<i64>(47).map(|v| v != 0)?,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_union_find_clustering() {
        let mut parent = std::collections::HashMap::new();

        fn find(parent: &mut std::collections::HashMap<i64, i64>, x: i64) -> i64 {
            if !parent.contains_key(&x) {
                parent.insert(x, x);
                return x;
            }
            let mut root = x;
            while parent[&root] != root {
                root = parent[&root];
            }
            root
        }

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
