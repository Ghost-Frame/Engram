use std::collections::HashSet;
use super::types::{GraphBuildOptions, GraphEdge, GraphNode, LinkType};
use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};
use tracing::info;

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

pub async fn build_graph(db: &Database) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    let result = build_graph_data(db, &GraphBuildOptions::default()).await?;
    Ok((result.nodes, result.edges))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphBuildResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

pub async fn build_graph_data(
    db: &Database,
    opts: &GraphBuildOptions,
) -> Result<GraphBuildResult> {
    let conn = db.connection();
    let user_id = opts.user_id;
    let limit = opts.limit.unwrap_or(1000) as i64;

    // Phase 1: Top memories ordered by importance + pagerank
    let mut mem_rows = conn
        .query(
            "SELECT id, content, category, importance, pagerank_score \
             FROM memories \
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
             ORDER BY COALESCE(pagerank_score, 0.0) + CAST(COALESCE(importance, 5) AS REAL) * 0.1 DESC \
             LIMIT ?2",
            libsql::params![user_id, limit],
        )
        .await?;

    struct MemRow {
        id: i64,
        content: String,
        category: String,
        importance: i64,
        pagerank: f64,
    }

    let mut memory_data: Vec<MemRow> = Vec::new();
    while let Some(row) = mem_rows.next().await? {
        memory_data.push(MemRow {
            id: row.get(0)?,
            content: row.get(1)?,
            category: row.get::<String>(2).unwrap_or_else(|_| "general".to_string()),
            importance: row.get::<i64>(3).unwrap_or(5),
            pagerank: row.get::<f64>(4).unwrap_or(0.0),
        });
    }

    if memory_data.is_empty() {
        return Ok(GraphBuildResult { nodes: vec![], edges: vec![] });
    }

    let mem_set: HashSet<i64> = memory_data.iter().map(|m| m.id).collect();

    // Phase 2: Build nodes
    let mut nodes: Vec<GraphNode> = memory_data
        .iter()
        .map(|m| GraphNode {
            id: format!("m{}", m.id),
            label: truncate_label(&m.content, 60),
            weight: ((m.importance as f32 * 1.5) + (m.pagerank as f32 * 5.0)).max(3.0),
            pagerank: Some(m.pagerank as f32),
            community: None,
            metadata: Some(serde_json::json!({ "category": m.category })),
        })
        .collect();

    // Phase 3: Fetch links scoped to user, filter to collected memory set in Rust
    let mut link_rows = conn
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

    let mut edges: Vec<GraphEdge> = Vec::new();
    while let Some(row) = link_rows.next().await? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        if !mem_set.contains(&source_id) || !mem_set.contains(&target_id) {
            continue;
        }
        let similarity: f64 = row.get(2)?;
        let type_str: String = row
            .get::<String>(3)
            .unwrap_or_else(|_| "similarity".to_string());
        edges.push(GraphEdge {
            source: format!("m{}", source_id),
            target: format!("m{}", target_id),
            link_type: link_type_from_str(&type_str),
            weight: similarity as f32,
        });
    }

    // Phase 4: Prune orphan memory nodes (no edges)
    let connected: HashSet<String> = edges
        .iter()
        .flat_map(|e| [e.source.clone(), e.target.clone()])
        .collect();
    nodes.retain(|n| connected.contains(&n.id));

    info!(user_id, nodes = nodes.len(), edges = edges.len(), "graph_built");

    Ok(GraphBuildResult { nodes, edges })
}
