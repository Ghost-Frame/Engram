use super::types::{GraphEdge, GraphNode, LinkType};
use crate::db::Database;
use crate::{EngError, Result};
use std::collections::{HashMap, HashSet};

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
#[tracing::instrument(skip(db, query), fields(query_len = query.len()))]
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
                     WHERE is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 \
                       AND content LIKE ?1 \
                     ORDER BY importance DESC \
                     LIMIT ?2",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(rusqlite::params![pattern_clone, limit as i64], |row| {
                    let id: i64 = row.get(0)?;
                    let content: String = row.get(1)?;
                    let category: String =
                        row.get::<_, String>(2).unwrap_or_else(|_| "general".into());
                    let importance: i64 = row.get(3)?;
                    let pagerank: f64 = row.get::<_, Option<f64>>(4)?.unwrap_or(0.0);
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

            let mut nodes = Vec::new();
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
                     WHERE (name LIKE ?1 OR aliases LIKE ?1 OR description LIKE ?1) \
                     ORDER BY occurrence_count DESC \
                     LIMIT ?2",
                )
                .map_err(rusqlite_to_eng_error)?;

            let rows = stmt
                .query_map(rusqlite::params![pattern, limit as i64], |row| {
                    let id: i64 = row.get(0)?;
                    let name: String = row.get(1)?;
                    Ok((id, name))
                })
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

/// BFS neighborhood traversal from a start node (3.12 enhanced).
///
/// Expands outward through memory_links up to `depth` hops.
/// Batches DB queries per hop level (not per node) to avoid N+1.
/// Optional `link_types` filter restricts traversal to specific edge types.
/// Returns the subgraph of visited nodes and traversed edges, plus
/// a per-node hop distance map in the response.
#[tracing::instrument(skip(db, node_id))]
pub async fn neighborhood(
    db: &Database,
    node_id: &str,
    depth: u32,
    user_id: i64,
) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    let (nodes, edges, _hops) = neighborhood_filtered(db, node_id, depth, user_id, None).await?;
    Ok((nodes, edges))
}

/// BFS neighborhood with optional link_type filter.
#[allow(clippy::type_complexity)]
#[tracing::instrument(skip(db, node_id, link_types))]
pub async fn neighborhood_filtered(
    db: &Database,
    node_id: &str,
    depth: u32,
    user_id: i64,
    link_types: Option<&[String]>,
) -> Result<(Vec<GraphNode>, Vec<GraphEdge>, HashMap<String, u32>)> {
    // Parse node_id: "m123" -> memory id 123, "e456" -> entity id 456
    let (node_type, raw_id) = if let Some(stripped) = node_id.strip_prefix('m') {
        ("memory", stripped.parse::<i64>().unwrap_or(0))
    } else if let Some(stripped) = node_id.strip_prefix('e') {
        ("entity", stripped.parse::<i64>().unwrap_or(0))
    } else {
        return Ok((Vec::new(), Vec::new(), HashMap::new()));
    };

    if raw_id == 0 {
        return Ok((Vec::new(), Vec::new(), HashMap::new()));
    }

    // Only memory traversal supported for now
    if node_type != "memory" {
        return Ok((Vec::new(), Vec::new(), HashMap::new()));
    }

    let mut visited: HashSet<i64> = HashSet::new();
    let mut frontier: Vec<i64> = Vec::new();
    let mut all_edges: Vec<GraphEdge> = Vec::new();
    let mut all_node_ids: Vec<i64> = Vec::new();
    let mut hop_map: HashMap<i64, u32> = HashMap::new();

    visited.insert(raw_id);
    frontier.push(raw_id);
    all_node_ids.push(raw_id);
    hop_map.insert(raw_id, 0);

    // Pre-compute link type filter for SQL
    let type_filter: Option<Vec<String>> = link_types.map(|lt| lt.to_vec());

    for current_depth in 0..depth {
        if frontier.is_empty() {
            break;
        }

        // Batch fetch all edges for the entire frontier in one query
        let frontier_clone = frontier.clone();
        let type_filter_clone = type_filter.clone();
        let edges: Vec<(i64, i64, f64, String)> = db
            .read(move |conn| {
                // Build IN clause placeholders
                let placeholders: String = frontier_clone
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format!("?{}", i + 1))
                    .collect::<Vec<_>>()
                    .join(",");

                let type_clause = if let Some(ref types) = type_filter_clone {
                    let type_placeholders: String = types
                        .iter()
                        .enumerate()
                        .map(|(i, _)| format!("?{}", frontier_clone.len() + 2 + i))
                        .collect::<Vec<_>>()
                        .join(",");
                    format!(" AND ml.type IN ({})", type_placeholders)
                } else {
                    String::new()
                };

                let sql = format!(
                    "SELECT ml.source_id, ml.target_id, ml.similarity, ml.type \
                     FROM memory_links ml \
                     WHERE (ml.source_id IN ({placeholders}) OR ml.target_id IN ({placeholders})) \
                       AND EXISTS (SELECT 1 FROM memories WHERE id = ml.source_id) \
                       AND EXISTS (SELECT 1 FROM memories WHERE id = ml.target_id){type_clause}",
                    placeholders = placeholders,
                    type_clause = type_clause,
                );

                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;

                // R8 P-005: borrow params directly instead of Box::new +
                // clone. frontier_clone and type_filter_clone outlive
                // this block so &i64 / &String refs are stable.
                let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(
                    frontier_clone.len() + type_filter_clone.as_ref().map_or(0, |t| t.len()),
                );
                for id in &frontier_clone {
                    params.push(id);
                }
                if let Some(ref types) = type_filter_clone {
                    for t in types {
                        params.push(t);
                    }
                }

                let rows = stmt
                    .query_map(params.as_slice(), |row: &rusqlite::Row| {
                        let source_id: i64 = row.get(0)?;
                        let target_id: i64 = row.get(1)?;
                        let similarity: f64 = row.get(2)?;
                        let link_type_str: String = row
                            .get::<_, Option<String>>(3)?
                            .unwrap_or_else(|| "cite".to_string());
                        Ok((source_id, target_id, similarity, link_type_str))
                    })
                    .map_err(rusqlite_to_eng_error)?;

                let mut result: Vec<(i64, i64, f64, String)> = Vec::new();
                for row in rows {
                    result.push(row.map_err(rusqlite_to_eng_error)?);
                }
                Ok(result)
            })
            .await?;

        // Process edges and build next frontier
        let mut next_frontier: Vec<i64> = Vec::new();

        for (source_id, target_id, similarity, link_type_str) in edges {
            all_edges.push(GraphEdge {
                source: format!("m{}", source_id),
                target: format!("m{}", target_id),
                link_type: parse_link_type(&link_type_str),
                weight: similarity as f32,
            });

            for &neighbor in &[source_id, target_id] {
                if !visited.contains(&neighbor) {
                    visited.insert(neighbor);
                    next_frontier.push(neighbor);
                    all_node_ids.push(neighbor);
                    hop_map.insert(neighbor, current_depth + 1);
                }
            }
        }

        frontier = next_frontier;
    }

    // Batch fetch node details for all collected IDs in one query
    let nodes = batch_fetch_memory_nodes(db, &all_node_ids).await?;

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

    // Build string-keyed hop map for response
    let string_hop_map: HashMap<String, u32> = hop_map
        .into_iter()
        .map(|(id, hop)| (format!("m{}", id), hop))
        .collect();

    Ok((nodes, all_edges, string_hop_map))
}

/// Batch fetch memory node details for a list of IDs in a single query.
async fn batch_fetch_memory_nodes(db: &Database, ids: &[i64]) -> Result<Vec<GraphNode>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let ids_owned: Vec<i64> = ids.to_vec();
    let rows: Vec<MemoryNodeRow> = db
        .read(move |conn| {
            let placeholders: String = ids_owned
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");

            let sql = format!(
                "SELECT id, content, category, importance, pagerank_score, \
                        source, created_at, is_static, source_count, \
                        decay_score, community_id \
                 FROM memories WHERE id IN ({})",
                placeholders
            );

            let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;

            let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
            for &id in &ids_owned {
                params.push(Box::new(id));
            }

            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                params.iter().map(|p| p.as_ref()).collect();

            let mapped = stmt
                .query_map(param_refs.as_slice(), |row: &rusqlite::Row| {
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

            let mut result: Vec<MemoryNodeRow> = Vec::new();
            for row in mapped {
                result.push(row.map_err(rusqlite_to_eng_error)?);
            }
            Ok(result)
        })
        .await?;

    let mut nodes = Vec::with_capacity(rows.len());
    for r in rows {
        let label = if r.content.len() > 60 {
            format!(
                "{}...",
                &r.content[..r
                    .content
                    .char_indices()
                    .nth(60)
                    .map_or(r.content.len(), |(i, _)| i)]
            )
        } else {
            r.content.clone()
        };

        let size = r.importance as f32 * 1.5 + r.pagerank as f32 * 5.0;

        nodes.push(GraphNode {
            id: format!("m{}", r.id),
            label,
            weight: size,
            pagerank: Some(r.pagerank as f32),
            community: r.community_id,
            metadata: None,
            node_type: "memory".into(),
            category: r.category.clone(),
            importance: r.importance,
            group: r.category,
            size,
            source: r.source,
            created_at: r.created_at,
            is_static: r.is_static,
            content: r.content,
            source_count: r.source_count,
            community_id: r.community_id,
            decay_score: r.decay_score,
        });
    }

    Ok(nodes)
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
