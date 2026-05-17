//! High-level structural-analysis operations consumed by the HTTP routes
//! and MCP tools. Each function takes an EN source string, parses it into a
//! graph, runs the relevant algorithm, and returns a JSON-friendly report.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::graph::{Graph, NodeRole, Topology};
use super::parser::parse_en_source;
use crate::{EngError, Result};

/// Single node entry in the analyze report's `roles` list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRoleEntry {
    pub node: String,
    pub role: NodeRole,
    pub in_degree: usize,
    pub out_degree: usize,
}

/// Bridge edge in the underlying undirected graph (single point of failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeInfo {
    pub from: String,
    pub to: String,
}

/// Top-level analyze report mirroring `structural_analyze`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzeReport {
    pub node_count: usize,
    pub edge_count: usize,
    pub component_count: usize,
    pub topology: Topology,
    pub roles: Vec<NodeRoleEntry>,
    pub bridges: Vec<BridgeInfo>,
}

/// Extended analysis with concurrency metrics, critical path length, depth
/// levels, and bridge-implications hints. Cyclic graphs report `None` for
/// the depth-derived metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailReport {
    pub analyze: AnalyzeReport,
    pub critical_path_length: Option<usize>,
    pub flow_depth: Option<BTreeMap<String, usize>>,
    pub concurrency_by_level: Option<BTreeMap<usize, usize>>,
    pub max_concurrency: Option<usize>,
    pub bridge_implications: Vec<String>,
}

/// Shortest-path result for two named nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistanceReport {
    pub from: String,
    pub to: String,
    pub distance: Option<usize>,
    pub path: Option<Vec<String>>,
}

/// Trace report walks the directed edge first; falls back to undirected and
/// flags the reversed hops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceReport {
    pub from: String,
    pub to: String,
    pub path: Option<Vec<String>>,
    pub used_undirected: bool,
    pub reverse_edges: Vec<BridgeInfo>,
}

fn ensure_node_exists(graph: &Graph, name: &str) -> Result<usize> {
    graph
        .index
        .get(name)
        .copied()
        .ok_or_else(|| EngError::InvalidInput(format!("node not found: {name}")))
}

/// Build the report shape consumed by `structural_analyze`.
pub fn analyze_source(source: &str) -> AnalyzeReport {
    let stmts = parse_en_source(source);
    let g = Graph::from_statements(&stmts);
    let topology = g.classify_topology();
    let mut roles = Vec::with_capacity(g.node_count());
    for i in 0..g.node_count() {
        roles.push(NodeRoleEntry {
            node: g.names[i].clone(),
            role: g.node_role(i),
            in_degree: g.in_edges[i].len(),
            out_degree: g.out_edges[i].len(),
        });
    }
    let bridges = g
        .bridges()
        .into_iter()
        .map(|(from, to)| BridgeInfo { from, to })
        .collect();
    AnalyzeReport {
        node_count: g.node_count(),
        edge_count: g.edge_count(),
        component_count: g.component_count(),
        topology,
        roles,
        bridges,
    }
}

/// `structural_detail` -- adds concurrency, critical-path, and bridge text.
pub fn detail_source(source: &str) -> DetailReport {
    let stmts = parse_en_source(source);
    let g = Graph::from_statements(&stmts);

    let analyze = AnalyzeReport {
        node_count: g.node_count(),
        edge_count: g.edge_count(),
        component_count: g.component_count(),
        topology: g.classify_topology(),
        roles: (0..g.node_count())
            .map(|i| NodeRoleEntry {
                node: g.names[i].clone(),
                role: g.node_role(i),
                in_degree: g.in_edges[i].len(),
                out_degree: g.out_edges[i].len(),
            })
            .collect(),
        bridges: g
            .bridges()
            .into_iter()
            .map(|(from, to)| BridgeInfo { from, to })
            .collect(),
    };

    let critical_path_length = g.critical_path_length();
    let flow_depth = g.flow_depth();
    let concurrency_by_level = g.concurrency_by_level();
    let max_concurrency = concurrency_by_level
        .as_ref()
        .and_then(|m| m.values().copied().max());

    let bridge_implications = analyze
        .bridges
        .iter()
        .map(|b| {
            format!(
                "Edge {}->{} is a single point of failure; removing it splits the system.",
                b.from, b.to
            )
        })
        .collect();

    DetailReport {
        analyze,
        critical_path_length,
        flow_depth,
        concurrency_by_level,
        max_concurrency,
        bridge_implications,
    }
}

/// `structural_between` -- betweenness centrality for one named node.
pub fn node_betweenness_in_source(source: &str, node: &str) -> Result<f64> {
    let stmts = parse_en_source(source);
    let g = Graph::from_statements(&stmts);
    ensure_node_exists(&g, node)?;
    let scores = g.betweenness();
    Ok(scores.get(node).copied().unwrap_or(0.0))
}

/// `structural_distance` -- directional shortest path; falls back to
/// undirected when no directed path exists and flags it in `path`.
pub fn distance_in_source(source: &str, from: &str, to: &str) -> Result<DistanceReport> {
    let stmts = parse_en_source(source);
    let g = Graph::from_statements(&stmts);
    let src = ensure_node_exists(&g, from)?;
    let dst = ensure_node_exists(&g, to)?;
    let path = g.bfs_path(src, dst);
    let distance = path.as_ref().map(|p| p.len().saturating_sub(1));
    Ok(DistanceReport {
        from: from.to_string(),
        to: to.to_string(),
        distance,
        path,
    })
}

/// `structural_trace` -- prefers the directed BFS path; if none exists,
/// falls back to undirected BFS and records every reversed hop so the
/// caller can see where the trace flowed against an arrow.
pub fn trace_in_source(source: &str, from: &str, to: &str) -> Result<TraceReport> {
    let stmts = parse_en_source(source);
    let g = Graph::from_statements(&stmts);
    let src = ensure_node_exists(&g, from)?;
    let dst = ensure_node_exists(&g, to)?;
    if let Some(path) = g.bfs_path(src, dst) {
        return Ok(TraceReport {
            from: from.to_string(),
            to: to.to_string(),
            path: Some(path),
            used_undirected: false,
            reverse_edges: Vec::new(),
        });
    }
    if let Some((path, reversed)) = g.bfs_path_undirected(src, dst) {
        let reverse_edges = reversed
            .into_iter()
            .map(|(from, to)| BridgeInfo { from, to })
            .collect();
        return Ok(TraceReport {
            from: from.to_string(),
            to: to.to_string(),
            path: Some(path),
            used_undirected: true,
            reverse_edges,
        });
    }
    Ok(TraceReport {
        from: from.to_string(),
        to: to.to_string(),
        path: None,
        used_undirected: false,
        reverse_edges: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_reports_topology_and_roles() {
        let rep = analyze_source("A yields: x. B needs: x yields: y. C needs: y.");
        assert_eq!(rep.node_count, 3);
        assert_eq!(rep.edge_count, 2);
        assert!(matches!(rep.topology, Topology::Pipeline));
        assert!(rep
            .roles
            .iter()
            .any(|r| r.node == "B" && matches!(r.role, NodeRole::Pipeline)));
        // Every pipeline edge is a bridge.
        assert_eq!(rep.bridges.len(), 2);
    }

    #[test]
    fn detail_includes_critical_path() {
        let rep = detail_source("A yields: x. B needs: x yields: y. C needs: y.");
        assert_eq!(rep.critical_path_length, Some(2));
        let depths = rep.flow_depth.unwrap();
        assert_eq!(depths["A"], 0);
        assert_eq!(depths["B"], 1);
        assert_eq!(depths["C"], 2);
    }

    #[test]
    fn between_intermediate_node_scores_higher() {
        let mid = node_betweenness_in_source("A yields: x. B needs: x yields: y. C needs: y.", "B")
            .unwrap();
        let sink =
            node_betweenness_in_source("A yields: x. B needs: x yields: y. C needs: y.", "C")
                .unwrap();
        assert!(mid > sink);
    }

    #[test]
    fn distance_reports_path_length() {
        let rep =
            distance_in_source("A yields: x. B needs: x yields: y. C needs: y.", "A", "C").unwrap();
        assert_eq!(rep.distance, Some(2));
        assert_eq!(rep.path.unwrap(), vec!["A", "B", "C"]);
    }

    #[test]
    fn trace_falls_back_to_undirected_and_flags_reverse() {
        let rep =
            trace_in_source("A yields: x. B needs: x yields: y. C needs: y.", "C", "A").unwrap();
        assert!(rep.used_undirected);
        assert_eq!(rep.path.unwrap(), vec!["C", "B", "A"]);
        assert_eq!(rep.reverse_edges.len(), 2);
    }

    #[test]
    fn missing_node_errors() {
        let err = node_betweenness_in_source("A yields: x.", "Z").unwrap_err();
        assert!(matches!(err, EngError::InvalidInput(_)));
    }
}
