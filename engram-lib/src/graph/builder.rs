use super::types::{GraphBuildOptions, GraphEdge, GraphNode, LinkType};
use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphBuildResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// Build the full graph for the default user.
pub async fn build_graph(db: &Database) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    let opts = GraphBuildOptions::default();
    let result = build_graph_data(db, &opts).await?;
    Ok((result.nodes, result.edges))
}

/// Build a graph from the user's memory space.
///
/// Phase 1: Fetch memory IDs (top-scored, limited by opts.limit)
/// Phase 2: Build nodes from memory metadata
/// Phase 3: Batch fetch links as edges
/// Phase 4: Prune orphan memory nodes (no edges)
pub async fn build_graph_data(
    db: &Database,
    opts: &GraphBuildOptions,
) -> Result<GraphBuildResult> {
    let conn = db.connection();
    let limit = opts.limit.unwrap_or(500) as i64;
    let user_id = opts.user_id;

    // -- Phase 1: Collect top-scored memory nodes ---------------------------------
    let mut rows = conn
        .query(
            "SELECT id, content, category, importance, pagerank_score \
             FROM memories \
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
             ORDER BY COALESCE(decay_score, importance) DESC \
             LIMIT ?2",
            libsql::params![user_id, limit],
        )
        .await?;

    let mut nodes = Vec::new();
    let mut memory_ids: Vec<i64> = Vec::new();

    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        let _category: String = row.get(2)?;
        let importance: i64 = row.get(3)?;
        let pagerank: f64 = row.get::<f64>(4).unwrap_or(0.0);

        let label = if content.len() > 60 {
            format!("{}...", &content[..content.char_indices().nth(60).map_or(content.len(), |(i, _)| i)])
        } else {
            content
        };

        nodes.push(GraphNode {
            id: format!("m{}", id),
            label,
            weight: importance as f32 * 1.5 + pagerank as f32 * 5.0,
            pagerank: Some(pagerank as f32),
            community: None,
            metadata: None,
        });
        memory_ids.push(id);
    }

    if memory_ids.is_empty() {
        return Ok(GraphBuildResult {
            nodes: Vec::new(),
            edges: Vec::new(),
        });
    }

    // -- Phase 2: Batch fetch links as edges --------------------------------------
    let placeholders: String = std::iter::repeat_n("?", memory_ids.len())
        .collect::<Vec<_>>()
        .join(", ");

    let query = format!(
        "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
         FROM memory_links ml \
         WHERE ml.source_id IN ({placeholders}) OR ml.target_id IN ({placeholders})"
    );

    let all_ids: Vec<libsql::Value> = memory_ids
        .iter()
        .chain(memory_ids.iter())
        .map(|&id| libsql::Value::Integer(id))
        .collect();

    let mut edge_rows = conn.query(&query, all_ids).await?;

    let valid_set: HashSet<i64> = memory_ids.iter().copied().collect();
    let mut edges = Vec::new();

    while let Some(row) = edge_rows.next().await? {
        let source_id: i64 = row.get(0)?;
        let target_id: i64 = row.get(1)?;
        let similarity: f64 = row.get(2)?;
        let link_type_str: String = row.get::<String>(3).unwrap_or_else(|_| "cite".to_string());

        if !valid_set.contains(&source_id) || !valid_set.contains(&target_id) {
            continue;
        }

        let link_type = parse_link_type(&link_type_str);

        edges.push(GraphEdge {
            source: format!("m{}", source_id),
            target: format!("m{}", target_id),
            link_type,
            weight: similarity as f32,
        });
    }

    // -- Phase 3: Prune orphan memory nodes (no edges) ----------------------------
    let connected_ids: HashSet<String> = edges
        .iter()
        .flat_map(|e| [e.source.clone(), e.target.clone()])
        .collect();

    nodes.retain(|n| connected_ids.contains(&n.id));

    info!(
        nodes = nodes.len(),
        edges = edges.len(),
        user_id,
        "graph_built"
    );

    Ok(GraphBuildResult { nodes, edges })
}

fn parse_link_type(s: &str) -> LinkType {
    match s {
        "cite" | "similarity" | "related" => LinkType::Cite,
        "mentions" | "about" => LinkType::Mentions,
        "association" | "Association" => LinkType::Association,
        "temporal" | "Temporal" => LinkType::Temporal,
        "contradicts" | "contradiction" | "Contradiction" => LinkType::Contradicts,
        "causal" | "causes" | "caused_by" | "Causal" => LinkType::Causal,
        "resolves" | "Resolves" => LinkType::Resolves,
        "refines" | "updates" | "corrects" => LinkType::Refines,
        "generalizes" | "consolidates" => LinkType::Generalizes,
        "has_fact" => LinkType::HasFact,
        _ => LinkType::Cite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_build_result_structure() {
        let result = GraphBuildResult {
            nodes: vec![GraphNode {
                id: "m1".to_string(),
                label: "test memory".to_string(),
                weight: 7.5,
                pagerank: Some(0.5),
                community: None,
                metadata: None,
            }],
            edges: vec![GraphEdge {
                source: "m1".to_string(),
                target: "m2".to_string(),
                link_type: LinkType::Cite,
                weight: 0.9,
            }],
        };
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(result.nodes[0].id, "m1");
    }

    #[test]
    fn test_parse_link_type() {
        assert_eq!(parse_link_type("cite"), LinkType::Cite);
        assert_eq!(parse_link_type("similarity"), LinkType::Cite);
        assert_eq!(parse_link_type("contradicts"), LinkType::Contradicts);
        assert_eq!(parse_link_type("has_fact"), LinkType::HasFact);
        assert_eq!(parse_link_type("updates"), LinkType::Refines);
        assert_eq!(parse_link_type("consolidates"), LinkType::Generalizes);
        assert_eq!(parse_link_type("unknown_type"), LinkType::Cite);
    }

    #[test]
    fn test_label_truncation() {
        let long_content = "a".repeat(100);
        let label = if long_content.len() > 60 {
            format!(
                "{}...",
                &long_content[..long_content.char_indices().nth(60).map_or(long_content.len(), |(i, _)| i)]
            )
        } else {
            long_content.clone()
        };
        assert_eq!(label.len(), 63); // 60 chars + "..."
    }
}
