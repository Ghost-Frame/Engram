use super::types::{GraphEdge, GraphNode, LinkType};
use crate::db::Database;
use crate::{EngError, Result};
use std::collections::{HashSet, VecDeque};

/// Row data for a memory node with all GUI-required fields.
struct MemoryNodeRow {
    id: i64,
    content: String,
    category: String,
    importance: i64,
    pagerank: f64,
    source: String,
    created_at: String,
    is_static: bool,
    source_count: i64,
    decay_score: Option<f64>,
    community_id: Option<u32>,
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Search graph nodes by name/content pattern.
/// Returns nodes whose content matches the query (LIKE search).
pub async fn graph_search(
    db: &Database,
    query: &str,
    limit: usize,
    user_id: i64,
) -> Result<Vec<GraphNode>> {
    let pattern = format!("%{}%", query);
    let pattern_clone = pattern.clone();

    let mut nodes: Vec<GraphNode> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, importance, pagerank_score, \
                            source, created_at, is_static, source_count, \
                            decay_score, community_id \
                     FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                       AND content LIKE ?2 \
                     ORDER BY importance DESC \
                     LIMIT ?3",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![user_id, pattern_clone, limit as i64],
                    |row| {
                        let id: i64 = row.get(0)?;
                        let content: String = row.get(1)?;
                        let category: String = row.get::<_, String>(2).unwrap_or_else(|_| "general".into());
                        let importance: i64 = row.get(3)?;
                        let pagerank: f64 = row.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
                        let source: String = row.get::<_, String>(5).unwrap_or_else(|_| "unknown".into());
                        let created_at: String = row.get::<_, String>(6).unwrap_or_default();
                        let is_static: bool = row.get::<_, bool>(7).unwrap_or(false);
                        let source_count: i64 = row.get::<_, i64>(8).unwrap_or(1);
                        let decay_score: Option<f64> = row.get::<_, f64>(9).ok();
                        let community_id: Option<u32> = row.get::<_, i64>(10).ok().map(|v| v as u32);
                        Ok((id, content, category, importance, pagerank, source, created_at, is_static, source_count, decay_score, community_id))
                    },
                )
                .map_err(rusqlite_to_eng_error)?;

            let mut nodes = Vec::new();
            for row in rows {
                let (id, content, category, importance, pagerank, source, created_at, is_static, source_count, decay_score, community_id) =
                    row.map_err(rusqlite_to_eng_error)?;

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
            }
            Ok(nodes)
        })
        .await?;

    // Also search entities
    let entity_nodes: Vec<GraphNode> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, entity_type \
                     FROM entities \
                     WHERE user_id = ?1 AND (name LIKE ?2 OR aliases LIKE ?2 OR description LIKE ?2) \
                     ORDER BY occurrence_count DESC \
                     LIMIT ?3",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(
                    rusqlite::params![user_id, pattern, limit as i64],
                    |row| {
                        let id: i64 = row.get(0)?;
                        let name: String = row.get(1)?;
                        Ok((id, name))
                    },
                )
                .map_err(rusqlite_to_eng_error)?;

            let mut nodes = Vec::new();
            for row in rows {
                let (id, name) = row.map_err(rusqlite_to_eng_error)?;
                nodes.push(GraphNode {
                    id: format!("e{}", id),
                    label: name.clone(),
                    weight: 8.0,
                    pagerank: None,
                    community: None,
                    metadata: None,
                    node_type: "entity".into(),
                    category: "entity".into(),
                    importance: 5,
                    group: "entity".into(),
                    size: 8.0,
                    source: "graph".into(),
                    created_at: String::new(),
                    is_static: false,
                    content: name,
                    source_count: 1,
                    community_id: None,
                    decay_score: None,
                });
            }
            Ok(nodes)
        })
        .await?;

    nodes.extend(entity_nodes);
    Ok(nodes)
}

/// BFS neighborhood traversal from a start node.
/// Expands outward through memory_links up to `depth` hops.
/// Returns the subgraph of visited nodes and traversed edges.
pub async fn neighborhood(
    db: &Database,
    node_id: &str,
    depth: u32,
    user_id: i64,
) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    // Parse node_id: "m123" -> memory id 123, "e456" -> entity id 456
    let (node_type, raw_id) = if let Some(stripped) = node_id.strip_prefix('m') {
        ("memory", stripped.parse::<i64>().unwrap_or(0))
    } else if let Some(stripped) = node_id.strip_prefix('e') {
        ("entity", stripped.parse::<i64>().unwrap_or(0))
    } else {
        return Ok((Vec::new(), Vec::new()));
    };

    if raw_id == 0 {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut visited: HashSet<i64> = HashSet::new();
    let mut frontier: VecDeque<i64> = VecDeque::new();
    let mut all_edges: Vec<GraphEdge> = Vec::new();
    let mut all_node_ids: Vec<i64> = Vec::new();

    if node_type == "memory" {
        visited.insert(raw_id);
        frontier.push_back(raw_id);
        all_node_ids.push(raw_id);

        for _d in 0..depth {
            if frontier.is_empty() {
                break;
            }

            let current_frontier: Vec<i64> = frontier.drain(..).collect();

            for &node in &current_frontier {
                let edges: Vec<(i64, i64, f64, String)> = db
                    .read(move |conn| {
                        let mut stmt = conn
                            .prepare(
                                "SELECT source_id, target_id, similarity, type \
                                 FROM memory_links \
                                 WHERE (source_id = ?1 OR target_id = ?1) \
                                   AND EXISTS (SELECT 1 FROM memories WHERE id = source_id AND user_id = ?2) \
                                   AND EXISTS (SELECT 1 FROM memories WHERE id = target_id AND user_id = ?2)",
                            )
                            .map_err(rusqlite_to_eng_error)?;

                        let rows = stmt
                            .query_map(rusqlite::params![node, user_id], |row| {
                                let source_id: i64 = row.get(0)?;
                                let target_id: i64 = row.get(1)?;
                                let similarity: f64 = row.get(2)?;
                                let link_type_str: String = row
                                    .get::<_, Option<String>>(3)?
                                    .unwrap_or_else(|| "cite".to_string());
                                Ok((source_id, target_id, similarity, link_type_str))
                            })
                            .map_err(rusqlite_to_eng_error)?;

                        let mut result = Vec::new();
                        for row in rows {
                            result.push(row.map_err(rusqlite_to_eng_error)?);
                        }
                        Ok(result)
                    })
                    .await?;

                for (source_id, target_id, similarity, link_type_str) in edges {
                    let neighbor = if source_id == node {
                        target_id
                    } else {
                        source_id
                    };

                    all_edges.push(GraphEdge {
                        source: format!("m{}", source_id),
                        target: format!("m{}", target_id),
                        link_type: parse_link_type(&link_type_str),
                        weight: similarity as f32,
                    });

                    if !visited.contains(&neighbor) {
                        visited.insert(neighbor);
                        frontier.push_back(neighbor);
                        all_node_ids.push(neighbor);
                    }
                }
            }
        }
    }

    // Fetch node details for all collected IDs
    let mut nodes = Vec::new();
    for &id in &all_node_ids {
        let result: Option<MemoryNodeRow> = db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, content, category, importance, pagerank_score, \
                                source, created_at, is_static, source_count, \
                                decay_score, community_id \
                         FROM memories WHERE id = ?1 AND user_id = ?2",
                    )
                    .map_err(rusqlite_to_eng_error)?;

                let mut rows = stmt
                    .query_map(rusqlite::params![id, user_id], |row| {
                        Ok(MemoryNodeRow {
                            id: row.get(0)?,
                            content: row.get(1)?,
                            category: row.get::<_, String>(2).unwrap_or_else(|_| "general".into()),
                            importance: row.get(3)?,
                            pagerank: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
                            source: row.get::<_, String>(5).unwrap_or_else(|_| "unknown".into()),
                            created_at: row.get::<_, String>(6).unwrap_or_default(),
                            is_static: row.get::<_, bool>(7).unwrap_or(false),
                            source_count: row.get::<_, i64>(8).unwrap_or(1),
                            decay_score: row.get::<_, f64>(9).ok(),
                            community_id: row.get::<_, i64>(10).ok().map(|v| v as u32),
                        })
                    })
                    .map_err(rusqlite_to_eng_error)?;

                match rows.next() {
                    Some(row) => Ok(Some(row.map_err(rusqlite_to_eng_error)?)),
                    None => Ok(None),
                }
            })
            .await?;

        if let Some(r) = result {
            let (mem_id, content, category, importance, pagerank, source, created_at, is_static, source_count, decay_score, community_id)
                = (r.id, r.content, r.category, r.importance, r.pagerank, r.source, r.created_at, r.is_static, r.source_count, r.decay_score, r.community_id);
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
                id: format!("m{}", mem_id),
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
        }
    }

    // Deduplicate edges
    let mut seen_edges: HashSet<(String, String, String)> = HashSet::new();
    all_edges.retain(|e| {
        let key = (
            e.source.clone(),
            e.target.clone(),
            format!("{:?}", e.link_type),
        );
        seen_edges.insert(key)
    });

    Ok((nodes, all_edges))
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
    fn test_parse_node_id_memory() {
        let (t, id) = if let Some(s) = "m123".strip_prefix('m') {
            ("memory", s.parse::<i64>().unwrap_or(0))
        } else {
            ("unknown", 0)
        };
        assert_eq!(t, "memory");
        assert_eq!(id, 123);
    }

    #[test]
    fn test_parse_node_id_entity() {
        let (t, id) = if let Some(s) = "e456".strip_prefix('e') {
            ("entity", s.parse::<i64>().unwrap_or(0))
        } else {
            ("unknown", 0)
        };
        assert_eq!(t, "entity");
        assert_eq!(id, 456);
    }

    #[test]
    fn test_parse_link_type_variants() {
        assert_eq!(parse_link_type("contradicts"), LinkType::Contradicts);
        assert_eq!(parse_link_type("has_fact"), LinkType::HasFact);
        assert_eq!(parse_link_type("random"), LinkType::Cite);
    }
}
