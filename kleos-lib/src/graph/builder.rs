use super::types::{GraphBuildOptions, GraphBuildResult, GraphEdge, GraphNode, LinkType};
use crate::db::Database;
use crate::{EngError, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::info;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Build the full graph for the default user.
#[tracing::instrument(skip(db))]
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
#[tracing::instrument(skip(db, opts), fields(user_id = opts.user_id, limit = ?opts.limit))]
pub async fn build_graph_data(db: &Database, opts: &GraphBuildOptions) -> Result<GraphBuildResult> {
    let limit = opts.limit.unwrap_or(500) as i64;
    let user_id = opts.user_id;

    // -- Phase 1: Collect top-scored memory nodes ---------------------------------
    let (nodes, memory_ids) = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance, pagerank_score, \
                            source, created_at, is_static, source_count, \
                            decay_score, community_id \
                     FROM memories \
                     WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                     ORDER BY COALESCE(decay_score, importance) DESC \
                     LIMIT ?1",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(rusqlite::params![limit], |row| {
                    let id: i64 = row.get(0)?;
                    let content: String = row.get(1)?;
                    let category: String =
                        row.get::<_, String>(2).unwrap_or_else(|_| "general".into());
                    let importance: i64 = row.get(3)?;
                    let pagerank: f64 = row.get::<_, f64>(4).unwrap_or(0.0);
                    let source: String =
                        row.get::<_, String>(5).unwrap_or_else(|_| "unknown".into());
                    let created_at: String = row.get::<_, String>(6).unwrap_or_default();
                    let is_static: bool = row.get::<_, bool>(7).unwrap_or(false);
                    let source_count: i64 = row.get::<_, i64>(8).unwrap_or(1);
                    let decay_score: Option<f64> = row.get::<_, f64>(9).ok();
                    let community_id: Option<u32> = row.get::<_, i64>(10).ok().map(|v| v as u32);
                    Ok((
                        id,
                        content,
                        category,
                        importance,
                        pagerank,
                        source,
                        created_at,
                        is_static,
                        source_count,
                        decay_score,
                        community_id,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?;

            let mut nodes: Vec<GraphNode> = Vec::new();
            let mut memory_ids: Vec<i64> = Vec::new();

            for row in rows {
                let (
                    id,
                    content,
                    category,
                    importance,
                    pagerank,
                    source,
                    created_at,
                    is_static,
                    source_count,
                    decay_score,
                    community_id,
                ) = row.map_err(rusqlite_to_eng_error)?;

                let label = if content.len() > 60 {
                    format!(
                        "{}...",
                        &content[..content
                            .char_indices()
                            .nth(60)
                            .map_or(content.len(), |(i, _)| i)]
                    )
                } else {
                    content.clone()
                };

                let size = importance as f32 * 1.5 + pagerank as f32 * 5.0;

                nodes.push(GraphNode {
                    id: format!("m{}", id),
                    label,
                    weight: size,
                    pagerank: Some(pagerank as f32),
                    community: community_id,
                    metadata: None,
                    node_type: "memory".into(),
                    category: category.clone(),
                    importance,
                    group: category,
                    size,
                    source,
                    created_at,
                    is_static,
                    content,
                    source_count,
                    community_id,
                    decay_score,
                });
                memory_ids.push(id);
            }

            Ok((nodes, memory_ids))
        })
        .await?;

    if memory_ids.is_empty() {
        return Ok(GraphBuildResult {
            nodes: Vec::new(),
            edges: Vec::new(),
        });
    }

    // -- Phase 2: Batch fetch links as edges --------------------------------------
    // Tenant isolation is provided by ResolvedDb routing (Phase 5+); the JOIN no
    // longer needs the user_id predicate because tenant shards contain only one
    // tenant's memories. On the legacy monolith path (user_id = 1), no tenant
    // boundary exists by design.
    let edges = db
        .read(move |conn| {
            let placeholders: String = std::iter::repeat_n("?", memory_ids.len())
                .collect::<Vec<_>>()
                .join(", ");

            let query = format!(
                "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
                 FROM memory_links ml \
                 JOIN memories ms ON ms.id = ml.source_id \
                 JOIN memories mt ON mt.id = ml.target_id \
                 WHERE ml.source_id IN ({placeholders}) OR ml.target_id IN ({placeholders})"
            );

<<<<<<< HEAD
            // Parameter list: ids twice (source IN, target IN)
            let mut params: Vec<rusqlite::types::Value> =
                Vec::with_capacity(memory_ids.len() * 2);
=======
            // Build parameter list: ids twice (source IN, target IN)
            let mut params: Vec<rusqlite::types::Value> = Vec::with_capacity(memory_ids.len() * 2);
>>>>>>> 35c6356 (style: apply cargo fmt across workspace (Phase 5 closeout))
            for &id in memory_ids.iter().chain(memory_ids.iter()) {
                params.push(rusqlite::types::Value::Integer(id));
            }

            let valid_set: HashSet<i64> = memory_ids.iter().copied().collect();

            let mut stmt = conn.prepare(&query).map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                    let source_id: i64 = row.get(0)?;
                    let target_id: i64 = row.get(1)?;
                    let similarity: f64 = row.get(2)?;
                    let link_type_str: String = row
                        .get::<_, String>(3)
                        .unwrap_or_else(|_| "cite".to_string());
                    Ok((source_id, target_id, similarity, link_type_str))
                })
                .map_err(rusqlite_to_eng_error)?;

            let mut edges: Vec<GraphEdge> = Vec::new();

            for row in rows {
                let (source_id, target_id, similarity, link_type_str) =
                    row.map_err(rusqlite_to_eng_error)?;

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

            Ok(edges)
        })
        .await?;

    // -- Phase 2b: Normalize edge weights to 0..1 ----------------------------------
    // Raw similarity scores cluster in a narrow band (e.g. 0.63-1.0 for cosine).
    // The GUI force simulation thresholds expect a full 0-1 range, so we min-max
    // normalize to spread them out.
    let mut edges = edges;
    if edges.len() > 1 {
        let min_w = edges.iter().map(|e| e.weight).fold(f32::INFINITY, f32::min);
        let max_w = edges
            .iter()
            .map(|e| e.weight)
            .fold(f32::NEG_INFINITY, f32::max);
        let range = max_w - min_w;
        if range > 0.001 {
            for edge in &mut edges {
                edge.weight = (edge.weight - min_w) / range;
            }
        }
    }

    // -- Phase 3: Keep only large connected components -----------------------------
    // Small disconnected clusters (< 10 nodes) scatter the force layout since
    // nothing links them to the main graph. BFS to find components, keep the big ones.
    let adj: HashMap<&str, Vec<&str>> = {
        let mut m: HashMap<&str, Vec<&str>> = HashMap::new();
        for e in &edges {
            m.entry(e.source.as_str())
                .or_default()
                .push(e.target.as_str());
            m.entry(e.target.as_str())
                .or_default()
                .push(e.source.as_str());
        }
        m
    };

    let node_ids: HashSet<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
    let mut visited: HashSet<&str> = HashSet::new();
    let mut keep_ids: HashSet<String> = HashSet::new();
    const MIN_COMPONENT_SIZE: usize = 10;

    for nid in &node_ids {
        if visited.contains(nid) {
            continue;
        }
        let mut component: Vec<&str> = Vec::new();
        let mut queue: VecDeque<&str> = VecDeque::new();
        queue.push_back(nid);
        while let Some(cur) = queue.pop_front() {
            if !visited.insert(cur) {
                continue;
            }
            component.push(cur);
            if let Some(neighbors) = adj.get(cur) {
                for nb in neighbors {
                    if node_ids.contains(nb) && !visited.contains(nb) {
                        queue.push_back(nb);
                    }
                }
            }
        }
        if component.len() >= MIN_COMPONENT_SIZE {
            for id in component {
                keep_ids.insert(id.to_string());
            }
        }
    }

    let mut nodes = nodes;
    nodes.retain(|n| keep_ids.contains(&n.id));
    edges.retain(|e| keep_ids.contains(&e.source) && keep_ids.contains(&e.target));

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
                node_type: "memory".into(),
                category: "general".into(),
                importance: 5,
                group: "general".into(),
                size: 7.5,
                source: "test".into(),
                created_at: "2026-01-01".into(),
                is_static: false,
                content: "test memory".into(),
                source_count: 1,
                community_id: None,
                decay_score: None,
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
                &long_content[..long_content
                    .char_indices()
                    .nth(60)
                    .map_or(long_content.len(), |(i, _)| i)]
            )
        } else {
            long_content.clone()
        };
        assert_eq!(label.len(), 63); // 60 chars + "..."
    }
}
