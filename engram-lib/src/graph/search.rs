use std::collections::HashSet;
use super::types::{GraphEdge, GraphNode, LinkType};
use crate::db::Database;
use crate::{EngError, Result};

fn link_type_from_str(s: &str) -> LinkType {
    match s {
        "contradicts" | "contradiction" => LinkType::Contradicts,
        "cite" | "cites" | "cited_by" => LinkType::Cite,
        "refines" | "refined_by" => LinkType::Refines,
        "generalizes" | "generalized_by" => LinkType::Generalizes,
        "has_fact" | "fact" => LinkType::HasFact,
        _ => LinkType::Mentions,
    }
}

fn truncate_label(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => format!("{}\u{2026}", &s[..i]),
        None => s.to_string(),
    }
}

/// Text search through memory content. Returns matching memories as GraphNodes,
/// ordered by importance descending.
pub async fn graph_search(
    db: &Database,
    query: &str,
    limit: usize,
    user_id: i64,
) -> Result<Vec<GraphNode>> {
    let conn = db.connection();
    let pattern = format!("%{}%", query);

    let mut rows = conn
        .query(
            "SELECT id, content, category, importance, pagerank_score \
             FROM memories \
             WHERE (content LIKE ?1 OR category LIKE ?1) \
               AND user_id = ?2 \
               AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
             ORDER BY importance DESC, COALESCE(pagerank_score, 0.0) DESC \
             LIMIT ?3",
            libsql::params![pattern, user_id, limit as i64],
        )
        .await?;

    let mut nodes = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        let category: String = row
            .get::<String>(2)
            .unwrap_or_else(|_| "general".to_string());
        let importance: i64 = row.get::<i64>(3).unwrap_or(5);
        let pagerank: f64 = row.get::<f64>(4).unwrap_or(0.0);

        nodes.push(GraphNode {
            id: format!("m{}", id),
            label: truncate_label(&content, 60),
            weight: ((importance as f32 * 1.5) + (pagerank as f32 * 5.0)).max(3.0),
            pagerank: Some(pagerank as f32),
            community: None,
            metadata: Some(serde_json::json!({ "category": category, "content": content })),
        });
    }

    Ok(nodes)
}

/// BFS neighborhood traversal from a memory node up to `depth` hops.
/// `node_id` accepts both raw numeric IDs and "m<id>" prefixed strings.
pub async fn neighborhood(
    db: &Database,
    node_id: &str,
    depth: u32,
    user_id: i64,
) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    let memory_id: i64 = node_id
        .trim_start_matches('m')
        .parse()
        .map_err(|_| EngError::NotFound(format!("invalid node id: {}", node_id)))?;

    let conn = db.connection();

    // BFS to collect all node IDs within `depth` hops
    let mut visited: HashSet<i64> = HashSet::new();
    visited.insert(memory_id);
    let mut frontier = vec![memory_id];

    for _ in 0..depth {
        if frontier.is_empty() {
            break;
        }
        let mut next_frontier: Vec<i64> = Vec::new();
        for &fid in &frontier {
            let mut rows = conn
                .query(
                    "SELECT CASE WHEN source_id = ?1 THEN target_id ELSE source_id END \
                     FROM memory_links \
                     WHERE source_id = ?1 OR target_id = ?1",
                    libsql::params![fid],
                )
                .await?;
            while let Some(row) = rows.next().await? {
                let nbr: i64 = row.get(0)?;
                if visited.insert(nbr) {
                    next_frontier.push(nbr);
                }
            }
        }
        frontier = next_frontier;
    }

    // Fetch all node details
    let mut nodes: Vec<GraphNode> = Vec::new();
    for &id in &visited {
        let mut rows = conn
            .query(
                "SELECT id, content, category, importance, pagerank_score \
                 FROM memories WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0",
                libsql::params![id, user_id],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let mid: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let category: String = row
                .get::<String>(2)
                .unwrap_or_else(|_| "general".to_string());
            let importance: i64 = row.get::<i64>(3).unwrap_or(5);
            let pagerank: f64 = row.get::<f64>(4).unwrap_or(0.0);

            nodes.push(GraphNode {
                id: format!("m{}", mid),
                label: truncate_label(&content, 60),
                weight: ((importance as f32 * 1.5) + (pagerank as f32 * 5.0)).max(3.0),
                pagerank: Some(pagerank as f32),
                community: None,
                metadata: Some(serde_json::json!({ "category": category })),
            });
        }
    }

    // Fetch all edges between visited nodes
    let mut edges: Vec<GraphEdge> = Vec::new();
    for &src_id in &visited {
        let mut rows = conn
            .query(
                "SELECT source_id, target_id, similarity, type \
                 FROM memory_links WHERE source_id = ?1",
                libsql::params![src_id],
            )
            .await?;
        while let Some(row) = rows.next().await? {
            let src: i64 = row.get(0)?;
            let tgt: i64 = row.get(1)?;
            if !visited.contains(&tgt) {
                continue;
            }
            let similarity: f64 = row.get(2)?;
            let type_str: String = row
                .get::<String>(3)
                .unwrap_or_else(|_| "similarity".to_string());
            edges.push(GraphEdge {
                source: format!("m{}", src),
                target: format!("m{}", tgt),
                link_type: link_type_from_str(&type_str),
                weight: similarity as f32,
            });
        }
    }

    Ok((nodes, edges))
}
