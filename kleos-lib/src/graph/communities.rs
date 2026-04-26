// ============================================================================
// COMMUNITY DETECTION via Louvain modularity optimization
// ============================================================================

use super::types::{CommunitiesResult, CommunityMember, CommunityStats};
use crate::db::Database;
use crate::{EngError, Result};
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
pub async fn detect_communities(
    db: &Database,
    user_id: i64,
    max_iterations: u32,
) -> Result<CommunitiesResult> {
    // SECURITY/DoS: Louvain modularity optimization runs O(n^2)-ish over the
    // node count and O(E) per pass over edges. Cap both so a large tenant
    // cannot run the server out of CPU and memory in a single call. Callers
    // hitting the cap still get a best-effort result over the top-N memories.
    const MAX_NODES: i64 = 10_000;
    const MAX_ITERATIONS: u32 = 100;
    let max_iterations = max_iterations.clamp(1, MAX_ITERATIONS);

    // --- Load memory ids ---
    let memory_ids: Vec<i64> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                     ORDER BY importance DESC, id DESC LIMIT ?2",
                )
                .map_err(rusqlite_to_eng_error)?;
            let ids = stmt
                .query_map(rusqlite::params![user_id, MAX_NODES], |row| row.get(0))
                .map_err(rusqlite_to_eng_error)?
                .collect::<std::result::Result<Vec<i64>, _>>()
                .map_err(rusqlite_to_eng_error)?;
            Ok(ids)
        })
        .await?;

    if memory_ids.is_empty() {
        return Ok(CommunitiesResult {
            communities: 0,
            memories: 0,
        });
    }

    // --- Load edges ---
    struct EdgeRow {
        source_id: i64,
        target_id: i64,
        similarity: f64,
        link_type: String,
    }

    let edges: Vec<EdgeRow> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
                     FROM memory_links ml \
                     JOIN memories ms ON ms.id = ml.source_id \
                     JOIN memories mt ON mt.id = ml.target_id \
                     WHERE ms.user_id = ?1 AND mt.user_id = ?1 \
                       AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
                       AND ms.is_archived = 0 AND mt.is_archived = 0",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows = stmt
                .query_map(rusqlite::params![user_id], |row| {
                    Ok(EdgeRow {
                        source_id: row.get(0)?,
                        target_id: row.get(1)?,
                        similarity: row.get(2)?,
                        link_type: row.get(3)?,
                    })
                })
                .map_err(rusqlite_to_eng_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(rusqlite_to_eng_error)?;
            Ok(rows)
        })
        .await?;

    let mem_set: std::collections::HashSet<i64> = memory_ids.iter().copied().collect();
    let mut adj: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    for &id in &memory_ids {
        adj.insert(id, HashMap::new());
    }
    for edge in edges {
        if !mem_set.contains(&edge.source_id) || !mem_set.contains(&edge.target_id) {
            continue;
        }
        let w = edge_weight(&edge.link_type, edge.similarity);
        adj.entry(edge.source_id)
            .or_default()
            .entry(edge.target_id)
            .and_modify(|e| *e += w)
            .or_insert(w);
        adj.entry(edge.target_id)
            .or_default()
            .entry(edge.source_id)
            .and_modify(|e| *e += w)
            .or_insert(w);
    }

    let m: f64 = adj
        .iter()
        .flat_map(|(k, vs)| vs.iter().filter(move |(v, _)| **v > *k).map(|(_, w)| w))
        .sum();

    if m == 0.0 {
        // No edges -- assign each memory its own community.
        let ids_clone = memory_ids.clone();
        db.transaction(move |tx| {
            for (idx, &id) in ids_clone.iter().enumerate() {
                tx.execute(
                    "UPDATE memories SET community_id = ?1 WHERE id = ?2",
                    rusqlite::params![idx as i64, id],
                )
                .map_err(rusqlite_to_eng_error)?;
            }
            Ok(())
        })
        .await?;

        return Ok(CommunitiesResult {
            communities: memory_ids.len(),
            memories: memory_ids.len(),
        });
    }

    let mut community: HashMap<i64, usize> = HashMap::new();
    for (i, &id) in memory_ids.iter().enumerate() {
        community.insert(id, i);
    }
    let mut k: HashMap<i64, f64> = HashMap::new();
    for &id in &memory_ids {
        k.insert(id, adj.get(&id).map(|v| v.values().sum()).unwrap_or(0.0));
    }
    let two_m = 2.0 * m;

    // Build sigma_tot (sum of k-values per community) once and maintain it
    // incrementally. The previous approach recomputed it by scanning all N
    // nodes inside the per-node inner loop, making each iteration O(N^2)
    // instead of O(N) (RB-L2).
    let mut sigma_tot: HashMap<usize, f64> = HashMap::new();
    for (&n, &c) in &community {
        *sigma_tot.entry(c).or_insert(0.0) += k.get(&n).copied().unwrap_or(0.0);
    }

    for _ in 0..max_iterations {
        let mut improved = false;
        for &node in &memory_ids {
            let node_comm = *community.get(&node).unwrap();
            let ki = *k.get(&node).unwrap();
            let mut comm_weights: HashMap<usize, f64> = HashMap::new();
            if let Some(neighbors) = adj.get(&node) {
                for (&nbr, &w) in neighbors {
                    *comm_weights
                        .entry(*community.get(&nbr).unwrap())
                        .or_insert(0.0) += w;
                }
            }
            let kic = comm_weights.get(&node_comm).copied().unwrap_or(0.0);
            let sc = sigma_tot.get(&node_comm).copied().unwrap_or(0.0);
            let dr = -kic / m + ki * (sc - ki) / (two_m * m);
            let mut bc = node_comm;
            let mut bd = 0.0;
            for (&tc, &kit) in &comm_weights {
                if tc == node_comm {
                    continue;
                }
                let stc = sigma_tot.get(&tc).copied().unwrap_or(0.0);
                let dt = dr + kit / m - ki * stc / (two_m * m);
                if dt > bd {
                    bd = dt;
                    bc = tc;
                }
            }
            if bc != node_comm && bd > 1e-10 {
                // Update sigma_tot incrementally: node moves from node_comm to bc.
                *sigma_tot.entry(node_comm).or_insert(0.0) -= ki;
                *sigma_tot.entry(bc).or_insert(0.0) += ki;
                community.insert(node, bc);
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }

    let mut label_map: HashMap<usize, i64> = HashMap::new();
    let mut next_community: i64 = 0;
    for &c in community.values() {
        if let std::collections::hash_map::Entry::Vacant(e) = label_map.entry(c) {
            e.insert(next_community);
            next_community += 1;
        }
    }

    // Collect (node_id, community_label) pairs to move into the closure.
    let updates: Vec<(i64, i64)> = community
        .iter()
        .map(|(&node_id, &comm)| (node_id, label_map.get(&comm).copied().unwrap_or(0)))
        .collect();

    db.transaction(move |tx| {
        for (node_id, cid) in &updates {
            tx.execute(
                "UPDATE memories SET community_id = ?1 WHERE id = ?2",
                rusqlite::params![cid, node_id],
            )
            .map_err(rusqlite_to_eng_error)?;
        }
        Ok(())
    })
    .await?;

    let num_communities = label_map.len();
    let mut comm_sizes: HashMap<i64, usize> = HashMap::new();
    for &c in community.values() {
        *comm_sizes
            .entry(label_map.get(&c).copied().unwrap_or(0))
            .or_insert(0) += 1;
    }
    let largest = comm_sizes.values().max().copied().unwrap_or(0);
    let isolated = comm_sizes.values().filter(|&&s| s == 1).count();
    info!(
        communities = num_communities,
        memories = memory_ids.len(),
        largest,
        isolated,
        user_id,
        "communities_detected"
    );
    Ok(CommunitiesResult {
        communities: num_communities,
        memories: memory_ids.len(),
    })
}

#[tracing::instrument(skip(db))]
pub async fn get_community_members(
    db: &Database,
    community_id: i64,
    user_id: i64,
    limit: usize,
) -> Result<Vec<CommunityMember>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, category, importance, created_at FROM memories \
                 WHERE community_id = ?1 AND user_id = ?2 AND is_forgotten = 0 AND is_archived = 0 \
                 ORDER BY importance DESC, created_at DESC LIMIT ?3",
            )
            .map_err(rusqlite_to_eng_error)?;
        let members = stmt
<<<<<<< HEAD
            .query_map(
                rusqlite::params![community_id, user_id, limit as i64],
                |row| {
                    Ok(CommunityMember {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        importance: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )
=======
            .query_map(rusqlite::params![community_id, limit as i64], |row| {
                Ok(CommunityMember {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    importance: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
>>>>>>> 35c6356 (style: apply cargo fmt across workspace (Phase 5 closeout))
            .map_err(rusqlite_to_eng_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(rusqlite_to_eng_error)?;
        Ok(members)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn get_community_stats(db: &Database, user_id: i64) -> Result<Vec<CommunityStats>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT community_id, COUNT(*) as count, ROUND(AVG(importance), 1) as avg_importance, \
                 GROUP_CONCAT(DISTINCT category) as categories \
                 FROM memories WHERE community_id IS NOT NULL AND is_forgotten = 0 AND is_archived = 0 \
                 GROUP BY community_id ORDER BY count DESC LIMIT 50",
            )
            .map_err(rusqlite_to_eng_error)?;
        let stats = stmt
            .query_map([], |row| {
                Ok(CommunityStats {
                    community_id: row.get(0)?,
                    count: row.get(1)?,
                    avg_importance: row.get::<_, f64>(2).unwrap_or(0.0),
                    categories: row.get::<_, String>(3).unwrap_or_default(),
                })
            })
            .map_err(rusqlite_to_eng_error)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(rusqlite_to_eng_error)?;
        Ok(stats)
    })
    .await
}
