// ============================================================================
// COMMUNITY DETECTION via Louvain modularity optimization
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
pub struct CommunitiesResult { pub communities: usize, pub memories: usize }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityMember { pub id: i64, pub content: String, pub category: String, pub importance: i64, pub created_at: String }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityStats { pub community_id: i64, pub count: i64, pub avg_importance: f64, pub categories: String }

pub async fn detect_communities(db: &Database, user_id: i64, max_iterations: u32) -> Result<CommunitiesResult> {
    let conn = db.connection();
    let mut mem_rows = conn.query(
        "SELECT id FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1",
        libsql::params![user_id]).await?;
    let mut memory_ids: Vec<i64> = Vec::new();
    while let Some(row) = mem_rows.next().await? { memory_ids.push(row.get(0)?); }
    if memory_ids.is_empty() { return Ok(CommunitiesResult { communities: 0, memories: 0 }); }

    let mut edge_rows = conn.query(
        "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
         FROM memory_links ml \
         JOIN memories ms ON ms.id = ml.source_id \
         JOIN memories mt ON mt.id = ml.target_id \
         WHERE ms.user_id = ?1 AND mt.user_id = ?1 \
           AND ms.is_forgotten = 0 AND mt.is_forgotten = 0 \
           AND ms.is_archived = 0 AND mt.is_archived = 0",
        libsql::params![user_id]).await?;

    let mem_set: std::collections::HashSet<i64> = memory_ids.iter().copied().collect();
    let mut adj: HashMap<i64, HashMap<i64, f64>> = HashMap::new();
    for &id in &memory_ids { adj.insert(id, HashMap::new()); }
    while let Some(row) = edge_rows.next().await? {
        let source_id: i64 = row.get(0)?; let target_id: i64 = row.get(1)?;
        let similarity: f64 = row.get(2)?; let link_type: String = row.get(3)?;
        if !mem_set.contains(&source_id) || !mem_set.contains(&target_id) { continue; }
        let w = edge_weight(&link_type, similarity);
        adj.entry(source_id).or_default().entry(target_id).and_modify(|e| *e += w).or_insert(w);
        adj.entry(target_id).or_default().entry(source_id).and_modify(|e| *e += w).or_insert(w);
    }

    let m: f64 = adj.iter().flat_map(|(k, vs)| vs.iter().filter(move |(v, _)| **v > *k).map(|(_, w)| w)).sum();
    if m == 0.0 {
        for (idx, &id) in memory_ids.iter().enumerate() {
            conn.execute("UPDATE memories SET community_id = ?1 WHERE id = ?2", libsql::params![idx as i64, id]).await?;
        }
        return Ok(CommunitiesResult { communities: memory_ids.len(), memories: memory_ids.len() });
    }

    let mut community: HashMap<i64, usize> = HashMap::new();
    for (i, &id) in memory_ids.iter().enumerate() { community.insert(id, i); }
    let mut k: HashMap<i64, f64> = HashMap::new();
    for &id in &memory_ids { k.insert(id, adj.get(&id).map(|v| v.values().sum()).unwrap_or(0.0)); }
    let two_m = 2.0 * m;

    for _ in 0..max_iterations {
        let mut improved = false;
        for &node in &memory_ids {
            let node_comm = *community.get(&node).unwrap();
            let ki = *k.get(&node).unwrap();
            let mut comm_weights: HashMap<usize, f64> = HashMap::new();
            if let Some(neighbors) = adj.get(&node) {
                for (&nbr, &w) in neighbors { *comm_weights.entry(*community.get(&nbr).unwrap()).or_insert(0.0) += w; }
            }
            let mut sigma_tot: HashMap<usize, f64> = HashMap::new();
            for (&n, &c) in &community { *sigma_tot.entry(c).or_insert(0.0) += k.get(&n).unwrap(); }
            let kic = comm_weights.get(&node_comm).copied().unwrap_or(0.0);
            let sc = sigma_tot.get(&node_comm).copied().unwrap_or(0.0);
            let dr = -kic / m + ki * (sc - ki) / (two_m * m);
            let mut bc = node_comm; let mut bd = 0.0;
            for (&tc, &kit) in &comm_weights {
                if tc == node_comm { continue; }
                let stc = sigma_tot.get(&tc).copied().unwrap_or(0.0);
                let dt = dr + kit / m - ki * stc / (two_m * m);
                if dt > bd { bd = dt; bc = tc; }
            }
            if bc != node_comm && bd > 1e-10 { community.insert(node, bc); improved = true; }
        }
        if !improved { break; }
    }

    let mut label_map: HashMap<usize, i64> = HashMap::new();
    let mut next_community: i64 = 0;
    for &c in community.values() {
        if !label_map.contains_key(&c) { label_map.insert(c, next_community); next_community += 1; }
    }
    for (&node_id, &comm) in &community {
        let cid = label_map.get(&comm).copied().unwrap_or(0);
        conn.execute("UPDATE memories SET community_id = ?1 WHERE id = ?2", libsql::params![cid, node_id]).await?;
    }

    let num_communities = label_map.len();
    let mut comm_sizes: HashMap<i64, usize> = HashMap::new();
    for &c in community.values() { *comm_sizes.entry(label_map.get(&c).copied().unwrap_or(0)).or_insert(0) += 1; }
    let largest = comm_sizes.values().max().copied().unwrap_or(0);
    let isolated = comm_sizes.values().filter(|&&s| s == 1).count();
    info!(communities = num_communities, memories = memory_ids.len(), largest, isolated, "communities_detected");
    Ok(CommunitiesResult { communities: num_communities, memories: memory_ids.len() })
}

pub async fn get_community_members(db: &Database, community_id: i64, user_id: i64, limit: usize) -> Result<Vec<CommunityMember>> {
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT id, content, category, importance, created_at FROM memories \
         WHERE community_id = ?1 AND user_id = ?2 AND is_forgotten = 0 AND is_archived = 0 \
         ORDER BY importance DESC, created_at DESC LIMIT ?3",
        libsql::params![community_id, user_id, limit as i64]).await?;
    let mut members = Vec::new();
    while let Some(row) = rows.next().await? {
        members.push(CommunityMember { id: row.get(0)?, content: row.get(1)?, category: row.get(2)?, importance: row.get(3)?, created_at: row.get(4)? });
    }
    Ok(members)
}

pub async fn get_community_stats(db: &Database, user_id: i64) -> Result<Vec<CommunityStats>> {
    let conn = db.connection();
    let mut rows = conn.query(
        "SELECT community_id, COUNT(*) as count, ROUND(AVG(importance), 1) as avg_importance, \
         GROUP_CONCAT(DISTINCT category) as categories \
         FROM memories WHERE user_id = ?1 AND community_id IS NOT NULL AND is_forgotten = 0 AND is_archived = 0 \
         GROUP BY community_id ORDER BY count DESC LIMIT 50",
        libsql::params![user_id]).await?;
    let mut stats = Vec::new();
    while let Some(row) = rows.next().await? {
        stats.push(CommunityStats { community_id: row.get(0)?, count: row.get(1)?,
            avg_importance: row.get::<f64>(2).unwrap_or(0.0), categories: row.get::<String>(3).unwrap_or_default() });
    }
    Ok(stats)
}
