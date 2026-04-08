use super::types::{GraphEdge, GraphNode, LinkType};
use crate::db::Database;
use crate::Result;
use std::collections::{HashSet, VecDeque};

/// Search graph nodes by name/content pattern.
/// Returns nodes whose content matches the query (LIKE search).
pub async fn graph_search(
    db: &Database,
    query: &str,
    limit: usize,
) -> Result<Vec<GraphNode>> {
    let conn = db.connection();
    let pattern = format!("%{}%", query);

    let mut rows = conn
        .query(
            "SELECT id, content, category, importance, pagerank_score \
             FROM memories \
             WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
               AND content LIKE ?1 \
             ORDER BY importance DESC \
             LIMIT ?2",
            libsql::params![pattern.clone(), limit as i64],
        )
        .await?;

    let mut nodes = Vec::new();

    while let Some(row) = rows.next().await? {
        let id: i64 = row.get(0)?;
        let content: String = row.get(1)?;
        let _category: String = row.get(2)?;
        let importance: i64 = row.get(3)?;
        let pagerank: f64 = row.get::<f64>(4).unwrap_or(0.0);

        let label = if content.len() > 60 {
            format!(
                "{}...",
                &content[..content.char_indices().nth(60).map_or(content.len(), |(i, _)| i)]
            )
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
    }

    // Also search entities
    let mut entity_rows = conn
        .query(
            "SELECT id, name, entity_type \
             FROM entities \
             WHERE name LIKE ?1 OR aliases LIKE ?1 OR description LIKE ?1 \
             ORDER BY occurrence_count DESC \
             LIMIT ?2",
            libsql::params![pattern, limit as i64],
        )
        .await?;

    while let Some(row) = entity_rows.next().await? {
        let id: i64 = row.get(0)?;
        let name: String = row.get(1)?;
        let _entity_type: String = row.get(2)?;

        nodes.push(GraphNode {
            id: format!("e{}", id),
            label: name,
            weight: 8.0,
            pagerank: None,
            community: None,
            metadata: None,
        });
    }

    Ok(nodes)
}

/// BFS neighborhood traversal from a start node.
/// Expands outward through memory_links up to `depth` hops.
/// Returns the subgraph of visited nodes and traversed edges.
pub async fn neighborhood(
    db: &Database,
    node_id: &str,
    depth: u32,
) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    let conn = db.connection();

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
                // Find all links from/to this node
                let mut link_rows = conn
                    .query(
                        "SELECT source_id, target_id, similarity, type \
                         FROM memory_links \
                         WHERE source_id = ?1 OR target_id = ?1",
                        libsql::params![node],
                    )
                    .await?;

                while let Some(row) = link_rows.next().await? {
                    let source_id: i64 = row.get(0)?;
                    let target_id: i64 = row.get(1)?;
                    let similarity: f64 = row.get(2)?;
                    let link_type_str: String =
                        row.get::<String>(3).unwrap_or_else(|_| "cite".to_string());

                    let neighbor = if source_id == node { target_id } else { source_id };

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
        let mut rows = conn
            .query(
                "SELECT id, content, importance, pagerank_score \
                 FROM memories WHERE id = ?1",
                libsql::params![id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let mem_id: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let importance: i64 = row.get(2)?;
            let pagerank: f64 = row.get::<f64>(3).unwrap_or(0.0);

            let label = if content.len() > 60 {
                format!(
                    "{}...",
                    &content[..content.char_indices().nth(60).map_or(content.len(), |(i, _)| i)]
                )
            } else {
                content
            };

            nodes.push(GraphNode {
                id: format!("m{}", mem_id),
                label,
                weight: importance as f32 * 1.5 + pagerank as f32 * 5.0,
                pagerank: Some(pagerank as f32),
                community: None,
                metadata: None,
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
        "contradicts" => LinkType::Contradicts,
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
