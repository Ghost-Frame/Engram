// ============================================================================
// STRUCTURAL ANALYSIS ENGINE
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ENAction {
    pub subject: String,
    pub action: String,
    pub needs: Vec<String>,
    pub yields: Vec<String>,
    pub subsystem: Option<String>,
}

pub fn parse_en(source: &str) -> Vec<ENAction> {
    let mut actions = Vec::new();
    let mut current_subsystem: Option<String> = None;

    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            current_subsystem = Some(line[1..line.len() - 1].trim().to_string());
            continue;
        }

        let do_idx = match line.find(" do:") {
            Some(idx) => idx,
            None => continue,
        };

        let subject = line[..do_idx].trim().to_string();
        let rest = line[do_idx + 4..].trim();

        let needs_idx = rest.find("needs:");
        let yields_idx = rest.find("yields:");

        let (action, needs_str, yields_str) = match (needs_idx, yields_idx) {
            (Some(ni), Some(yi)) => (
                rest[..ni].trim().to_string(),
                rest[ni + 6..yi].trim().to_string(),
                rest[yi + 7..].trim().to_string(),
            ),
            (Some(ni), None) => (
                rest[..ni].trim().to_string(),
                rest[ni + 6..].trim().to_string(),
                String::new(),
            ),
            (None, Some(yi)) => (
                rest[..yi].trim().to_string(),
                String::new(),
                rest[yi + 7..].trim().to_string(),
            ),
            (None, None) => (rest.to_string(), String::new(), String::new()),
        };

        let needs: Vec<String> = if needs_str.is_empty() {
            vec![]
        } else {
            needs_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        let yields: Vec<String> = if yields_str.is_empty() {
            vec![]
        } else {
            yields_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };

        actions.push(ENAction {
            subject,
            action,
            needs,
            yields,
            subsystem: current_subsystem.clone(),
        });
    }
    actions
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub label: String,
    pub subject: Option<String>,
    pub subsystem: Option<String>,
}

pub struct StructuralGraph {
    pub out_edges: HashMap<String, Vec<String>>,
    pub in_edges: HashMap<String, Vec<String>>,
    pub nodes: Vec<String>,
    pub node_map: HashMap<String, StructuralNode>,
    pub actions: Vec<ENAction>,
}

impl StructuralGraph {
    fn new() -> Self {
        Self {
            out_edges: HashMap::new(),
            in_edges: HashMap::new(),
            nodes: Vec::new(),
            node_map: HashMap::new(),
            actions: Vec::new(),
        }
    }
    fn add_node(&mut self, id: &str, node: StructuralNode) {
        if !self.out_edges.contains_key(id) {
            self.out_edges.insert(id.to_string(), Vec::new());
            self.in_edges.insert(id.to_string(), Vec::new());
            self.nodes.push(id.to_string());
            self.node_map.insert(id.to_string(), node);
        }
    }
    fn has_node(&self, id: &str) -> bool {
        self.out_edges.contains_key(id)
    }
    fn add_directed_edge(&mut self, from: &str, to: &str) {
        if let Some(edges) = self.out_edges.get(from) {
            if edges.contains(&to.to_string()) {
                return;
            }
        }
        self.out_edges
            .entry(from.to_string())
            .or_default()
            .push(to.to_string());
        self.in_edges
            .entry(to.to_string())
            .or_default()
            .push(from.to_string());
    }
    fn out_neighbors(&self, id: &str) -> &[String] {
        self.out_edges.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }
    fn in_neighbors(&self, id: &str) -> &[String] {
        self.in_edges.get(id).map(|v| v.as_slice()).unwrap_or(&[])
    }
    fn neighbors(&self, id: &str) -> HashSet<String> {
        let mut r = HashSet::new();
        for n in self.out_neighbors(id) {
            r.insert(n.clone());
        }
        for n in self.in_neighbors(id) {
            r.insert(n.clone());
        }
        r
    }
    fn in_degree(&self, id: &str) -> usize {
        self.in_edges.get(id).map(|v| v.len()).unwrap_or(0)
    }
    fn out_degree(&self, id: &str) -> usize {
        self.out_edges.get(id).map(|v| v.len()).unwrap_or(0)
    }
    fn order(&self) -> usize {
        self.nodes.len()
    }
    fn size(&self) -> usize {
        self.out_edges.values().map(|v| v.len()).sum()
    }
    fn copy(&self) -> Self {
        Self {
            out_edges: self.out_edges.clone(),
            in_edges: self.in_edges.clone(),
            nodes: self.nodes.clone(),
            node_map: self.node_map.clone(),
            actions: self.actions.clone(),
        }
    }
    fn drop_node(&mut self, id: &str) {
        if let Some(out_nbrs) = self.out_edges.remove(id) {
            for nbr in &out_nbrs {
                if let Some(il) = self.in_edges.get_mut(nbr) {
                    il.retain(|x| x != id);
                }
            }
        }
        if let Some(in_nbrs) = self.in_edges.remove(id) {
            for nbr in &in_nbrs {
                if let Some(ol) = self.out_edges.get_mut(nbr) {
                    ol.retain(|x| x != id);
                }
            }
        }
        self.nodes.retain(|n| n != id);
        self.node_map.remove(id);
    }
    fn drop_edge_between(&mut self, a: &str, b: &str) {
        if let Some(ol) = self.out_edges.get_mut(a) {
            ol.retain(|x| x != b);
        }
        if let Some(il) = self.in_edges.get_mut(b) {
            il.retain(|x| x != a);
        }
        if let Some(ol) = self.out_edges.get_mut(b) {
            ol.retain(|x| x != a);
        }
        if let Some(il) = self.in_edges.get_mut(a) {
            il.retain(|x| x != b);
        }
    }
    fn has_directed_edge(&self, from: &str, to: &str) -> bool {
        self.out_edges
            .get(from)
            .map(|v| v.contains(&to.to_string()))
            .unwrap_or(false)
    }
}

pub fn entity_id(name: &str) -> String {
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_")
}

pub fn build_graph(actions: &[ENAction]) -> StructuralGraph {
    let mut graph = StructuralGraph::new();
    graph.actions = actions.to_vec();
    for act in actions {
        let action_id = entity_id(&act.subject);
        if !graph.has_node(&action_id) {
            graph.add_node(
                &action_id,
                StructuralNode {
                    id: action_id.clone(),
                    node_type: "action".to_string(),
                    label: act.subject.clone(),
                    subject: Some(act.subject.clone()),
                    subsystem: act.subsystem.clone(),
                },
            );
        }
        for need in &act.needs {
            let nid = entity_id(need);
            if !graph.has_node(&nid) {
                graph.add_node(
                    &nid,
                    StructuralNode {
                        id: nid.clone(),
                        node_type: "entity".to_string(),
                        label: need.clone(),
                        subject: None,
                        subsystem: None,
                    },
                );
            }
            graph.add_directed_edge(&nid, &action_id);
        }
        for y in &act.yields {
            let yid = entity_id(y);
            if !graph.has_node(&yid) {
                graph.add_node(
                    &yid,
                    StructuralNode {
                        id: yid.clone(),
                        node_type: "entity".to_string(),
                        label: y.clone(),
                        subject: None,
                        subsystem: None,
                    },
                );
            }
            graph.add_directed_edge(&action_id, &yid);
        }
    }
    graph
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TopologyType {
    Pipeline,
    Tree,
    DAG,
    #[serde(rename = "Fork-Join")]
    ForkJoin,
    #[serde(rename = "Series-Parallel")]
    SeriesParallel,
    Cycle,
    Disconnected,
    #[serde(rename = "Single-Node")]
    SingleNode,
    Empty,
}

impl std::fmt::Display for TopologyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopologyType::Pipeline => write!(f, "Pipeline"),
            TopologyType::Tree => write!(f, "Tree"),
            TopologyType::DAG => write!(f, "DAG"),
            TopologyType::ForkJoin => write!(f, "Fork-Join"),
            TopologyType::SeriesParallel => write!(f, "Series-Parallel"),
            TopologyType::Cycle => write!(f, "Cycle"),
            TopologyType::Disconnected => write!(f, "Disconnected"),
            TopologyType::SingleNode => write!(f, "Single-Node"),
            TopologyType::Empty => write!(f, "Empty"),
        }
    }
}

fn has_cycle(graph: &StructuralGraph) -> bool {
    let mut visited = HashSet::new();
    let mut stack = HashSet::new();
    fn dfs(
        node: &str,
        graph: &StructuralGraph,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
    ) -> bool {
        visited.insert(node.to_string());
        stack.insert(node.to_string());
        for neighbor in graph.out_neighbors(node) {
            if !visited.contains(neighbor.as_str()) {
                if dfs(neighbor, graph, visited, stack) {
                    return true;
                }
            } else if stack.contains(neighbor.as_str()) {
                return true;
            }
        }
        stack.remove(node);
        false
    }
    for node in &graph.nodes {
        if !visited.contains(node.as_str()) && dfs(node, graph, &mut visited, &mut stack) {
            return true;
        }
    }
    false
}

fn connected_components(graph: &StructuralGraph) -> Vec<Vec<String>> {
    let mut visited = HashSet::new();
    let mut components = Vec::new();
    for node in &graph.nodes {
        if !visited.contains(node.as_str()) {
            let mut comp = Vec::new();
            let mut queue = VecDeque::new();
            visited.insert(node.clone());
            queue.push_back(node.clone());
            while let Some(current) = queue.pop_front() {
                comp.push(current.clone());
                for n in graph.neighbors(&current) {
                    if !visited.contains(n.as_str()) {
                        visited.insert(n.clone());
                        queue.push_back(n);
                    }
                }
            }
            components.push(comp);
        }
    }
    components
}

pub fn classify_topology(graph: &StructuralGraph) -> TopologyType {
    let node_count = graph.order();
    let edge_count = graph.size();
    if node_count == 0 {
        return TopologyType::Empty;
    }
    if node_count == 1 {
        return TopologyType::SingleNode;
    }
    let components = connected_components(graph);
    if components.len() > 1 {
        return TopologyType::Disconnected;
    }
    if has_cycle(graph) {
        return TopologyType::Cycle;
    }
    let is_pipeline = graph
        .nodes
        .iter()
        .all(|n| graph.in_degree(n) <= 1 && graph.out_degree(n) <= 1);
    if is_pipeline {
        return TopologyType::Pipeline;
    }
    let sources: Vec<&String> = graph
        .nodes
        .iter()
        .filter(|n| graph.in_degree(n) == 0)
        .collect();
    if edge_count == node_count - 1 && sources.len() == 1 {
        return TopologyType::Tree;
    }
    let has_fork = graph.nodes.iter().any(|n| graph.out_degree(n) > 1);
    let has_join = graph.nodes.iter().any(|n| graph.in_degree(n) > 1);
    if has_fork && has_join {
        return TopologyType::ForkJoin;
    }
    TopologyType::DAG
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeRole {
    SOURCE,
    SINK,
    HUB,
    FORK,
    JOIN,
    PIPELINE,
    CYCLE,
    ISOLATED,
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeRole::SOURCE => write!(f, "SOURCE"),
            NodeRole::SINK => write!(f, "SINK"),
            NodeRole::HUB => write!(f, "HUB"),
            NodeRole::FORK => write!(f, "FORK"),
            NodeRole::JOIN => write!(f, "JOIN"),
            NodeRole::PIPELINE => write!(f, "PIPELINE"),
            NodeRole::CYCLE => write!(f, "CYCLE"),
            NodeRole::ISOLATED => write!(f, "ISOLATED"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRoleInfo {
    pub id: String,
    pub label: String,
    pub role: NodeRole,
    #[serde(rename = "inDegree")]
    pub in_degree: usize,
    #[serde(rename = "outDegree")]
    pub out_degree: usize,
    pub subsystem: Option<String>,
}

pub fn classify_node_roles(graph: &StructuralGraph) -> Vec<NodeRoleInfo> {
    graph
        .nodes
        .iter()
        .map(|id| {
            let in_d = graph.in_degree(id);
            let out_d = graph.out_degree(id);
            let node = graph.node_map.get(id);
            let role = if in_d == 0 && out_d == 0 {
                NodeRole::ISOLATED
            } else if in_d == 0 {
                NodeRole::SOURCE
            } else if out_d == 0 {
                NodeRole::SINK
            } else if in_d >= 2 && out_d >= 2 {
                NodeRole::HUB
            } else if out_d >= 2 {
                NodeRole::FORK
            } else if in_d >= 2 {
                NodeRole::JOIN
            } else {
                NodeRole::PIPELINE
            };
            NodeRoleInfo {
                id: id.clone(),
                label: node.map(|n| n.label.clone()).unwrap_or_else(|| id.clone()),
                role,
                in_degree: in_d,
                out_degree: out_d,
                subsystem: node.and_then(|n| n.subsystem.clone()),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bridge {
    pub source: String,
    pub target: String,
    #[serde(rename = "sourceLabel")]
    pub source_label: String,
    #[serde(rename = "targetLabel")]
    pub target_label: String,
}

pub fn find_bridges(graph: &StructuralGraph) -> Vec<Bridge> {
    let mut bridges = Vec::new();
    let mut disc: HashMap<String, usize> = HashMap::new();
    let mut low: HashMap<String, usize> = HashMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut timer: usize = 0;

    #[allow(clippy::too_many_arguments)]
    fn dfs(
        u: &str,
        parent: Option<&str>,
        graph: &StructuralGraph,
        disc: &mut HashMap<String, usize>,
        low: &mut HashMap<String, usize>,
        visited: &mut HashSet<String>,
        timer: &mut usize,
        bridges: &mut Vec<Bridge>,
    ) {
        visited.insert(u.to_string());
        disc.insert(u.to_string(), *timer);
        low.insert(u.to_string(), *timer);
        *timer += 1;
        for v in graph.neighbors(u) {
            if parent.is_some() && v == parent.unwrap() {
                continue;
            }
            if !visited.contains(v.as_str()) {
                dfs(&v, Some(u), graph, disc, low, visited, timer, bridges);
                let low_v = *low.get(v.as_str()).unwrap();
                let low_u = *low.get(u).unwrap();
                low.insert(u.to_string(), low_u.min(low_v));
                if low_v > *disc.get(u).unwrap() {
                    bridges.push(Bridge {
                        source: u.to_string(),
                        target: v.clone(),
                        source_label: graph
                            .node_map
                            .get(u)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| u.to_string()),
                        target_label: graph
                            .node_map
                            .get(v.as_str())
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| v.clone()),
                    });
                }
            } else {
                let low_u = *low.get(u).unwrap();
                let disc_v = *disc.get(v.as_str()).unwrap();
                low.insert(u.to_string(), low_u.min(disc_v));
            }
        }
    }
    for node in &graph.nodes {
        if !visited.contains(node.as_str()) {
            dfs(
                node,
                None,
                graph,
                &mut disc,
                &mut low,
                &mut visited,
                &mut timer,
                &mut bridges,
            );
        }
    }
    bridges
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub topology: TopologyType,
    pub node_count: usize,
    pub edge_count: usize,
    pub nodes: Vec<NodeRoleInfo>,
    pub bridges: Vec<Bridge>,
    pub sources: Vec<String>,
    pub sinks: Vec<String>,
    pub hubs: Vec<String>,
    pub components: usize,
}

pub fn analyze_system(source: &str) -> AnalysisResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    let nodes = classify_node_roles(&graph);
    let bridges = find_bridges(&graph);
    let topology = classify_topology(&graph);
    let components = connected_components(&graph);
    AnalysisResult {
        topology,
        node_count: graph.order(),
        edge_count: graph.size(),
        sources: nodes
            .iter()
            .filter(|n| n.role == NodeRole::SOURCE)
            .map(|n| n.label.clone())
            .collect(),
        sinks: nodes
            .iter()
            .filter(|n| n.role == NodeRole::SINK)
            .map(|n| n.label.clone())
            .collect(),
        hubs: nodes
            .iter()
            .filter(|n| n.role == NodeRole::HUB)
            .map(|n| n.label.clone())
            .collect(),
        nodes,
        bridges,
        components: components.len(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthLevel {
    pub depth: usize,
    pub nodes: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeImplication {
    pub bridge: Bridge,
    pub disconnected_components: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailResult {
    pub topology: TopologyType,
    pub critical_path: Vec<String>,
    pub critical_path_length: usize,
    pub max_parallelism: usize,
    pub depth_levels: Vec<DepthLevel>,
    pub bridges: Vec<Bridge>,
    pub bridge_implications: Vec<BridgeImplication>,
}

fn topological_sort(graph: &StructuralGraph) -> Option<Vec<String>> {
    let mut in_degrees: HashMap<String, usize> = HashMap::new();
    for n in &graph.nodes {
        in_degrees.insert(n.clone(), graph.in_degree(n));
    }
    let mut queue: VecDeque<String> = VecDeque::new();
    for (n, d) in &in_degrees {
        if *d == 0 {
            queue.push_back(n.clone());
        }
    }
    let mut order = Vec::new();
    while let Some(node) = queue.pop_front() {
        order.push(node.clone());
        for neighbor in graph.out_neighbors(&node) {
            let nd = in_degrees.get_mut(neighbor).unwrap();
            *nd -= 1;
            if *nd == 0 {
                queue.push_back(neighbor.clone());
            }
        }
    }
    if order.len() == graph.order() {
        Some(order)
    } else {
        None
    }
}

pub fn detail_analysis(source: &str) -> DetailResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    let topology = classify_topology(&graph);
    let bridges = find_bridges(&graph);
    let mut depths: HashMap<String, usize> = HashMap::new();
    let sources: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| graph.in_degree(n) == 0)
        .cloned()
        .collect();
    let mut queue: VecDeque<String> = VecDeque::new();
    for s in &sources {
        depths.insert(s.clone(), 0);
        queue.push_back(s.clone());
    }
    while let Some(node) = queue.pop_front() {
        let d = *depths.get(&node).unwrap();
        for neighbor in graph.out_neighbors(&node) {
            let existing = depths.get(neighbor).copied().unwrap_or(0);
            if d + 1 > existing {
                depths.insert(neighbor.clone(), d + 1);
                queue.push_back(neighbor.clone());
            }
        }
    }
    let mut depth_groups: HashMap<usize, Vec<String>> = HashMap::new();
    for (node, d) in &depths {
        depth_groups.entry(*d).or_default().push(
            graph
                .node_map
                .get(node)
                .map(|n| n.label.clone())
                .unwrap_or_else(|| node.clone()),
        );
    }
    let mut depth_levels: Vec<DepthLevel> = depth_groups
        .into_iter()
        .map(|(depth, nodes)| DepthLevel { depth, nodes })
        .collect();
    depth_levels.sort_by_key(|l| l.depth);
    let max_parallelism = depth_levels
        .iter()
        .map(|l| l.nodes.len())
        .max()
        .unwrap_or(0);
    let mut critical_path: Vec<String> = Vec::new();
    if let Some(topo_order) = topological_sort(&graph) {
        let mut dist: HashMap<String, usize> = HashMap::new();
        let mut prev: HashMap<String, Option<String>> = HashMap::new();
        for n in &topo_order {
            dist.insert(n.clone(), 0);
            prev.insert(n.clone(), None);
        }
        for n in &topo_order {
            let d = *dist.get(n).unwrap();
            for neighbor in graph.out_neighbors(n) {
                if d + 1 > *dist.get(neighbor).unwrap() {
                    dist.insert(neighbor.clone(), d + 1);
                    prev.insert(neighbor.clone(), Some(n.clone()));
                }
            }
        }
        let mut max_node = topo_order[0].clone();
        let mut max_dist = 0;
        for (n, d) in &dist {
            if *d > max_dist {
                max_dist = *d;
                max_node = n.clone();
            }
        }
        let mut path = Vec::new();
        let mut cur: Option<String> = Some(max_node);
        while let Some(ref node) = cur {
            path.push(
                graph
                    .node_map
                    .get(node)
                    .map(|n| n.label.clone())
                    .unwrap_or_else(|| node.clone()),
            );
            cur = prev.get(node).and_then(|p| p.clone());
        }
        path.reverse();
        critical_path = path;
    }
    let bridge_implications: Vec<BridgeImplication> = bridges
        .iter()
        .map(|bridge| {
            let mut temp = graph.copy();
            temp.drop_edge_between(&bridge.source, &bridge.target);
            BridgeImplication {
                bridge: bridge.clone(),
                disconnected_components: connected_components(&temp).len(),
            }
        })
        .collect();
    let crit_len = critical_path.len();
    DetailResult {
        topology,
        critical_path,
        critical_path_length: crit_len,
        max_parallelism,
        depth_levels,
        bridges,
        bridge_implications,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentralityEntry {
    pub id: String,
    pub label: String,
    pub centrality: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BetweennessResult {
    pub node: String,
    pub label: String,
    pub centrality: f64,
    pub all_centralities: Vec<CentralityEntry>,
}

fn brandes_betweenness(graph: &StructuralGraph) -> HashMap<String, f64> {
    let n = graph.order();
    let mut cb: HashMap<String, f64> = HashMap::new();
    for node in &graph.nodes {
        cb.insert(node.clone(), 0.0);
    }
    for s in &graph.nodes {
        let mut stack: Vec<String> = Vec::new();
        let mut pred: HashMap<String, Vec<String>> = HashMap::new();
        let mut sigma: HashMap<String, f64> = HashMap::new();
        let mut dist: HashMap<String, i64> = HashMap::new();
        for t in &graph.nodes {
            pred.insert(t.clone(), Vec::new());
            sigma.insert(t.clone(), 0.0);
            dist.insert(t.clone(), -1);
        }
        sigma.insert(s.clone(), 1.0);
        dist.insert(s.clone(), 0);
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(s.clone());
        while let Some(v) = queue.pop_front() {
            stack.push(v.clone());
            let d_v = *dist.get(&v).unwrap();
            for w in graph.neighbors(&v) {
                if *dist.get(&w).unwrap() < 0 {
                    dist.insert(w.clone(), d_v + 1);
                    queue.push_back(w.clone());
                }
                if *dist.get(&w).unwrap() == d_v + 1 {
                    let sv = *sigma.get(&v).unwrap();
                    *sigma.get_mut(&w).unwrap() += sv;
                    pred.get_mut(&w).unwrap().push(v.clone());
                }
            }
        }
        let mut delta: HashMap<String, f64> = HashMap::new();
        for t in &graph.nodes {
            delta.insert(t.clone(), 0.0);
        }
        while let Some(w) = stack.pop() {
            for v in pred.get(&w).unwrap().clone() {
                let sv = *sigma.get(&v).unwrap();
                let sw = *sigma.get(&w).unwrap();
                let dw = *delta.get(&w).unwrap();
                if sw > 0.0 {
                    *delta.get_mut(&v).unwrap() += (sv / sw) * (1.0 + dw);
                }
            }
            if w != *s {
                *cb.get_mut(&w).unwrap() += *delta.get(&w).unwrap();
            }
        }
    }
    let norm = if n > 2 {
        ((n - 1) * (n - 2)) as f64
    } else {
        1.0
    };
    for val in cb.values_mut() {
        *val /= norm;
    }
    cb
}

pub fn compute_betweenness(source: &str, target_node: &str) -> BetweennessResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    let centralities = brandes_betweenness(&graph);
    let node_id = entity_id(target_node);
    let mut all_sorted: Vec<CentralityEntry> = centralities
        .iter()
        .map(|(id, c)| CentralityEntry {
            id: id.clone(),
            label: graph
                .node_map
                .get(id)
                .map(|n| n.label.clone())
                .unwrap_or_else(|| id.clone()),
            centrality: (*c * 10000.0).round() / 10000.0,
        })
        .collect();
    all_sorted.sort_by(|a, b| {
        b.centrality
            .partial_cmp(&a.centrality)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    BetweennessResult {
        node: node_id.clone(),
        label: graph
            .node_map
            .get(&node_id)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| target_node.to_string()),
        centrality: centralities.get(&node_id).copied().unwrap_or(0.0),
        all_centralities: all_sorted,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathNode {
    pub id: String,
    pub label: String,
    pub subsystem: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistanceResult {
    pub from: String,
    pub to: String,
    pub distance: Option<usize>,
    pub path: Option<Vec<PathNode>>,
    pub subsystem_crossings: usize,
}

fn bidirectional_bfs(graph: &StructuralGraph, from: &str, to: &str) -> Option<Vec<String>> {
    if from == to {
        return Some(vec![from.to_string()]);
    }
    let mut visited: HashMap<String, Option<String>> = HashMap::new();
    visited.insert(from.to_string(), None);
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(from.to_string());
    while let Some(node) = queue.pop_front() {
        if node == to {
            let mut path = Vec::new();
            let mut cur: Option<String> = Some(to.to_string());
            while let Some(ref c) = cur {
                path.push(c.clone());
                cur = visited.get(c).and_then(|p| p.clone());
            }
            path.reverse();
            return Some(path);
        }
        for n in graph.neighbors(&node) {
            if !visited.contains_key(&n) {
                visited.insert(n.clone(), Some(node.clone()));
                queue.push_back(n);
            }
        }
    }
    None
}

pub fn compute_distance(source: &str, from_node: &str, to_node: &str) -> DistanceResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    let from_id = entity_id(from_node);
    let to_id = entity_id(to_node);
    if !graph.has_node(&from_id) || !graph.has_node(&to_id) {
        return DistanceResult {
            from: from_node.to_string(),
            to: to_node.to_string(),
            distance: None,
            path: None,
            subsystem_crossings: 0,
        };
    }
    match bidirectional_bfs(&graph, &from_id, &to_id) {
        None => DistanceResult {
            from: from_node.to_string(),
            to: to_node.to_string(),
            distance: None,
            path: None,
            subsystem_crossings: 0,
        },
        Some(path) => {
            let pd: Vec<PathNode> = path
                .iter()
                .map(|id| PathNode {
                    id: id.clone(),
                    label: graph
                        .node_map
                        .get(id)
                        .map(|n| n.label.clone())
                        .unwrap_or_else(|| id.clone()),
                    subsystem: graph.node_map.get(id).and_then(|n| n.subsystem.clone()),
                })
                .collect();
            let mut crossings = 0;
            for i in 1..pd.len() {
                if let (Some(p), Some(c)) = (&pd[i - 1].subsystem, &pd[i].subsystem) {
                    if p != c {
                        crossings += 1;
                    }
                }
            }
            let distance = if pd.is_empty() { 0 } else { pd.len() - 1 };
            DistanceResult {
                from: from_node.to_string(),
                to: to_node.to_string(),
                distance: Some(distance),
                path: Some(pd),
                subsystem_crossings: crossings,
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracePathNode {
    pub id: String,
    pub label: String,
    pub role: NodeRole,
    pub subsystem: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverseEdge {
    pub from: String,
    pub to: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceResult {
    pub from: String,
    pub to: String,
    pub directed_path: Option<Vec<TracePathNode>>,
    pub undirected_fallback: bool,
    pub reverse_edges: Vec<ReverseEdge>,
}

fn directed_bfs(graph: &StructuralGraph, from: &str, to: &str) -> Option<Vec<String>> {
    let mut visited = HashSet::new();
    let mut prev: HashMap<String, Option<String>> = HashMap::new();
    let mut queue = VecDeque::new();
    visited.insert(from.to_string());
    prev.insert(from.to_string(), None);
    queue.push_back(from.to_string());
    while let Some(node) = queue.pop_front() {
        if node == to {
            let mut path = Vec::new();
            let mut cur: Option<String> = Some(to.to_string());
            while let Some(ref c) = cur {
                path.push(c.clone());
                cur = prev.get(c).and_then(|p| p.clone());
            }
            path.reverse();
            return Some(path);
        }
        for neighbor in graph.out_neighbors(&node) {
            if !visited.contains(neighbor) {
                visited.insert(neighbor.clone());
                prev.insert(neighbor.clone(), Some(node.clone()));
                queue.push_back(neighbor.clone());
            }
        }
    }
    None
}

pub fn trace_flow(source: &str, from_node: &str, to_node: &str) -> TraceResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    let roles = classify_node_roles(&graph);
    let role_map: HashMap<String, NodeRole> = roles
        .iter()
        .map(|r| (r.id.clone(), r.role.clone()))
        .collect();
    let from_id = entity_id(from_node);
    let to_id = entity_id(to_node);
    if !graph.has_node(&from_id) || !graph.has_node(&to_id) {
        return TraceResult {
            from: from_node.to_string(),
            to: to_node.to_string(),
            directed_path: None,
            undirected_fallback: false,
            reverse_edges: vec![],
        };
    }
    let make_tp = |path: &[String]| -> Vec<TracePathNode> {
        path.iter()
            .map(|id| TracePathNode {
                id: id.clone(),
                label: graph
                    .node_map
                    .get(id)
                    .map(|n| n.label.clone())
                    .unwrap_or_else(|| id.clone()),
                role: role_map.get(id).cloned().unwrap_or(NodeRole::PIPELINE),
                subsystem: graph.node_map.get(id).and_then(|n| n.subsystem.clone()),
            })
            .collect()
    };
    if let Some(dp) = directed_bfs(&graph, &from_id, &to_id) {
        return TraceResult {
            from: from_node.to_string(),
            to: to_node.to_string(),
            directed_path: Some(make_tp(&dp)),
            undirected_fallback: false,
            reverse_edges: vec![],
        };
    }
    match bidirectional_bfs(&graph, &from_id, &to_id) {
        None => TraceResult {
            from: from_node.to_string(),
            to: to_node.to_string(),
            directed_path: None,
            undirected_fallback: false,
            reverse_edges: vec![],
        },
        Some(path) => {
            let mut re = Vec::new();
            for i in 0..path.len().saturating_sub(1) {
                let a = &path[i];
                let b = &path[i + 1];
                if !graph.has_directed_edge(a, b) && graph.has_directed_edge(b, a) {
                    re.push(ReverseEdge {
                        from: graph
                            .node_map
                            .get(a)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| a.clone()),
                        to: graph
                            .node_map
                            .get(b)
                            .map(|n| n.label.clone())
                            .unwrap_or_else(|| b.clone()),
                    });
                }
            }
            TraceResult {
                from: from_node.to_string(),
                to: to_node.to_string(),
                directed_path: Some(make_tp(&path)),
                undirected_fallback: true,
                reverse_edges: re,
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactResult {
    pub removed_node: String,
    pub removed_label: String,
    pub original_components: usize,
    pub after_components: usize,
    pub disconnected_nodes: Vec<String>,
    pub topology_before: TopologyType,
    pub topology_after: TopologyType,
}

pub fn compute_impact(source: &str, target_node: &str) -> ImpactResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    let node_id = entity_id(target_node);
    if !graph.has_node(&node_id) {
        let c = connected_components(&graph);
        let t = classify_topology(&graph);
        return ImpactResult {
            removed_node: node_id,
            removed_label: target_node.to_string(),
            original_components: c.len(),
            after_components: c.len(),
            disconnected_nodes: vec![],
            topology_before: t.clone(),
            topology_after: t,
        };
    }
    let tb = classify_topology(&graph);
    let cb = connected_components(&graph);
    let mut temp = graph.copy();
    temp.drop_node(&node_id);
    let ta = classify_topology(&temp);
    let mut ca = connected_components(&temp);
    let mut disc = Vec::new();
    if ca.len() > cb.len() {
        ca.sort_by_key(|c| c.len());
        for component in ca.iter().take(ca.len().saturating_sub(1)) {
            for n in component {
                disc.push(
                    graph
                        .node_map
                        .get(n)
                        .map(|nd| nd.label.clone())
                        .unwrap_or_else(|| n.clone()),
                );
            }
        }
    }
    ImpactResult {
        removed_node: node_id.clone(),
        removed_label: graph
            .node_map
            .get(&node_id)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| target_node.to_string()),
        original_components: cb.len(),
        after_components: ca.len(),
        disconnected_nodes: disc,
        topology_before: tb,
        topology_after: ta,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleChange {
    pub node: String,
    pub role_a: NodeRole,
    pub role_b: NodeRole,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    pub topology_a: TopologyType,
    pub topology_b: TopologyType,
    pub topology_changed: bool,
    pub nodes_only_in_a: Vec<String>,
    pub nodes_only_in_b: Vec<String>,
    pub nodes_in_both: Vec<String>,
    pub role_changes: Vec<RoleChange>,
    pub edge_count_a: usize,
    pub edge_count_b: usize,
    pub bridge_count_a: usize,
    pub bridge_count_b: usize,
}

pub fn structural_diff(source_a: &str, source_b: &str) -> DiffResult {
    let ga = build_graph(&parse_en(source_a));
    let gb = build_graph(&parse_en(source_b));
    let ta = classify_topology(&ga);
    let tb = classify_topology(&gb);
    let na: HashSet<&String> = ga.nodes.iter().collect();
    let nb: HashSet<&String> = gb.nodes.iter().collect();
    let oa: Vec<String> = na
        .difference(&nb)
        .map(|n| {
            ga.node_map
                .get(*n)
                .map(|nd| nd.label.clone())
                .unwrap_or_else(|| (*n).clone())
        })
        .collect();
    let ob: Vec<String> = nb
        .difference(&na)
        .map(|n| {
            gb.node_map
                .get(*n)
                .map(|nd| nd.label.clone())
                .unwrap_or_else(|| (*n).clone())
        })
        .collect();
    let both: Vec<String> = na.intersection(&nb).map(|n| (*n).clone()).collect();
    let rma: HashMap<String, NodeRole> = classify_node_roles(&ga)
        .iter()
        .map(|r| (r.id.clone(), r.role.clone()))
        .collect();
    let rmb: HashMap<String, NodeRole> = classify_node_roles(&gb)
        .iter()
        .map(|r| (r.id.clone(), r.role.clone()))
        .collect();
    let rc: Vec<RoleChange> = both
        .iter()
        .filter(|n| rma.get(*n) != rmb.get(*n))
        .map(|n| RoleChange {
            node: ga
                .node_map
                .get(n)
                .map(|nd| nd.label.clone())
                .unwrap_or_else(|| n.clone()),
            role_a: rma.get(n).cloned().unwrap_or(NodeRole::ISOLATED),
            role_b: rmb.get(n).cloned().unwrap_or(NodeRole::ISOLATED),
        })
        .collect();
    DiffResult {
        topology_a: ta.clone(),
        topology_b: tb.clone(),
        topology_changed: ta != tb,
        nodes_only_in_a: oa,
        nodes_only_in_b: ob,
        nodes_in_both: both
            .iter()
            .map(|n| {
                ga.node_map
                    .get(n)
                    .map(|nd| nd.label.clone())
                    .unwrap_or_else(|| n.clone())
            })
            .collect(),
        role_changes: rc,
        edge_count_a: ga.size(),
        edge_count_b: gb.size(),
        bridge_count_a: find_bridges(&ga).len(),
        bridge_count_b: find_bridges(&gb).len(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolveResult {
    pub diff: DiffResult,
    pub new_bridges: Vec<Bridge>,
    pub eliminated_bridges: Vec<Bridge>,
}

pub fn evolve_system(source: &str, patch: &str) -> EvolveResult {
    let ao = parse_en(source);
    let ap = parse_en(patch);
    let mut merged: HashMap<String, ENAction> = HashMap::new();
    for a in &ao {
        merged.insert(entity_id(&a.subject), a.clone());
    }
    for a in &ap {
        merged.insert(entity_id(&a.subject), a.clone());
    }
    let ms: String = merged
        .values()
        .map(|a| {
            let mut l = format!("{} do: {}", a.subject, a.action);
            if !a.needs.is_empty() {
                l.push_str(&format!(" needs: {}", a.needs.join(", ")));
            }
            if !a.yields.is_empty() {
                l.push_str(&format!(" yields: {}", a.yields.join(", ")));
            }
            l
        })
        .collect::<Vec<_>>()
        .join("\n");
    let diff = structural_diff(source, &ms);
    let ob = find_bridges(&build_graph(&ao));
    let ma: Vec<ENAction> = merged.into_values().collect();
    let nb = find_bridges(&build_graph(&ma));
    let ok: HashSet<String> = ob
        .iter()
        .map(|b| format!("{}->{}", b.source, b.target))
        .collect();
    let nk: HashSet<String> = nb
        .iter()
        .map(|b| format!("{}->{}", b.source, b.target))
        .collect();
    EvolveResult {
        diff,
        new_bridges: nb
            .into_iter()
            .filter(|b| !ok.contains(&format!("{}->{}", b.source, b.target)))
            .collect(),
        eliminated_bridges: ob
            .into_iter()
            .filter(|b| !nk.contains(&format!("{}->{}", b.source, b.target)))
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsystemInfo {
    pub name: String,
    pub members: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorizeResult {
    pub subsystems: Vec<SubsystemInfo>,
    pub modularity: f64,
}

fn louvain_communities(graph: &StructuralGraph) -> (HashMap<String, usize>, f64) {
    let nodes: Vec<String> = graph.nodes.clone();
    let n = nodes.len();
    if n < 2 {
        let mut r = HashMap::new();
        for (i, node) in nodes.iter().enumerate() {
            r.insert(node.clone(), i);
        }
        return (r, 0.0);
    }
    let mut adj: HashMap<String, HashMap<String, f64>> = HashMap::new();
    for node in &nodes {
        adj.insert(node.clone(), HashMap::new());
    }
    for node in &nodes {
        for nbr in graph.neighbors(node) {
            if nodes.contains(&nbr) {
                adj.get_mut(node).unwrap().entry(nbr.clone()).or_insert(1.0);
                adj.get_mut(&nbr)
                    .unwrap()
                    .entry(node.clone())
                    .or_insert(1.0);
            }
        }
    }
    let m: f64 = adj
        .iter()
        .flat_map(|(k, vs)| vs.iter().filter(move |(v, _)| *v > k).map(|(_, w)| w))
        .sum();
    if m == 0.0 {
        let mut r = HashMap::new();
        for (i, node) in nodes.iter().enumerate() {
            r.insert(node.clone(), i);
        }
        return (r, 0.0);
    }
    let mut community: HashMap<String, usize> = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        community.insert(node.clone(), i);
    }
    let mut k: HashMap<String, f64> = HashMap::new();
    for node in &nodes {
        k.insert(
            node.clone(),
            adj.get(node).map(|v| v.values().sum()).unwrap_or(0.0),
        );
    }
    let two_m = 2.0 * m;
    for _ in 0..50 {
        let mut improved = false;
        for node in &nodes {
            let nc = *community.get(node).unwrap();
            let ki = *k.get(node).unwrap();
            let mut cw: HashMap<usize, f64> = HashMap::new();
            if let Some(neighbors) = adj.get(node) {
                for (nbr, w) in neighbors {
                    *cw.entry(*community.get(nbr).unwrap()).or_insert(0.0) += w;
                }
            }
            let mut st: HashMap<usize, f64> = HashMap::new();
            for (n2, c2) in &community {
                *st.entry(*c2).or_insert(0.0) += k.get(n2).unwrap();
            }
            let kic = cw.get(&nc).copied().unwrap_or(0.0);
            let sc = st.get(&nc).copied().unwrap_or(0.0);
            let dr = -kic / m + ki * (sc - ki) / (two_m * m);
            let mut bc = nc;
            let mut bd = 0.0;
            for (&tc, &kit) in &cw {
                if tc == nc {
                    continue;
                }
                let stc = st.get(&tc).copied().unwrap_or(0.0);
                let dt = dr + kit / m - ki * stc / (two_m * m);
                if dt > bd {
                    bd = dt;
                    bc = tc;
                }
            }
            if bc != nc && bd > 1e-10 {
                community.insert(node.clone(), bc);
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }
    let mut q = 0.0;
    for (node, neighbors) in &adj {
        for (nbr, w) in neighbors {
            if node < nbr && community.get(node) == community.get(nbr) {
                q += w - (k.get(node).unwrap() * k.get(nbr).unwrap()) / two_m;
            }
        }
    }
    q /= m;
    (community, (q * 10000.0).round() / 10000.0)
}

pub fn categorize_system(source: &str) -> CategorizeResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    if graph.order() < 2 {
        let members: Vec<String> = graph
            .nodes
            .iter()
            .map(|n| {
                graph
                    .node_map
                    .get(n)
                    .map(|nd| nd.label.clone())
                    .unwrap_or_else(|| n.clone())
            })
            .collect();
        return CategorizeResult {
            subsystems: vec![SubsystemInfo {
                name: "System".to_string(),
                members,
            }],
            modularity: 0.0,
        };
    }
    let (communities, modularity) = louvain_communities(&graph);
    let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
    for (node, comm) in &communities {
        groups.entry(*comm).or_default().push(
            graph
                .node_map
                .get(node)
                .map(|n| n.label.clone())
                .unwrap_or_else(|| node.clone()),
        );
    }
    let mut subsystems: Vec<SubsystemInfo> = groups
        .into_iter()
        .map(|(id, members)| SubsystemInfo {
            name: format!("Subsystem-{}", id),
            members,
        })
        .collect();
    subsystems.sort_by_key(|s| s.name.clone());
    CategorizeResult {
        subsystems,
        modularity,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractResult {
    pub subsystem: String,
    pub source: String,
    pub boundary_inputs: Vec<String>,
    pub boundary_outputs: Vec<String>,
    pub internal_entities: Vec<String>,
}

pub fn extract_subsystem(source: &str, subsystem_name: &str) -> ExtractResult {
    let actions = parse_en(source);
    let graph = build_graph(&actions);
    let CategorizeResult { subsystems, .. } = categorize_system(source);
    let target = subsystems.iter().find(|s| {
        s.name.to_lowercase() == subsystem_name.to_lowercase()
            || s.members
                .iter()
                .any(|m| m.to_lowercase().contains(&subsystem_name.to_lowercase()))
    });
    let target = match target {
        Some(t) => t,
        None => {
            return ExtractResult {
                subsystem: subsystem_name.to_string(),
                source: String::new(),
                boundary_inputs: vec![],
                boundary_outputs: vec![],
                internal_entities: vec![],
            }
        }
    };
    let member_ids: HashSet<String> = target.members.iter().map(|m| entity_id(m)).collect();
    let sub_actions: Vec<&ENAction> = actions
        .iter()
        .filter(|a| member_ids.contains(&entity_id(&a.subject)))
        .collect();
    let all_needs: HashSet<String> = sub_actions
        .iter()
        .flat_map(|a| a.needs.iter().map(|n| entity_id(n)))
        .collect();
    let all_yields: HashSet<String> = sub_actions
        .iter()
        .flat_map(|a| a.yields.iter().map(|y| entity_id(y)))
        .collect();
    let bi: Vec<String> = all_needs
        .iter()
        .filter(|n| !member_ids.contains(*n) && !all_yields.contains(*n))
        .map(|n| {
            graph
                .node_map
                .get(n)
                .map(|nd| nd.label.clone())
                .unwrap_or_else(|| n.clone())
        })
        .collect();
    let bo: Vec<String> = all_yields
        .iter()
        .filter(|y| !member_ids.contains(*y) && !all_needs.contains(*y))
        .map(|y| {
            graph
                .node_map
                .get(y)
                .map(|nd| nd.label.clone())
                .unwrap_or_else(|| y.clone())
        })
        .collect();
    let mut seen = HashSet::new();
    let ie: Vec<String> = all_needs
        .union(&all_yields)
        .filter(|e| {
            let label = graph
                .node_map
                .get(*e)
                .map(|nd| nd.label.clone())
                .unwrap_or_else(|| (*e).clone());
            !bi.contains(&label) && !bo.contains(&label) && seen.insert((*e).clone())
        })
        .map(|e| {
            graph
                .node_map
                .get(e)
                .map(|nd| nd.label.clone())
                .unwrap_or_else(|| e.clone())
        })
        .collect();
    let ss: String = sub_actions
        .iter()
        .map(|a| {
            let mut l = format!("{} do: {}", a.subject, a.action);
            if !a.needs.is_empty() {
                l.push_str(&format!(" needs: {}", a.needs.join(", ")));
            }
            if !a.yields.is_empty() {
                l.push_str(&format!(" yields: {}", a.yields.join(", ")));
            }
            l
        })
        .collect::<Vec<_>>()
        .join("\n");
    ExtractResult {
        subsystem: target.name.clone(),
        source: ss,
        boundary_inputs: bi,
        boundary_outputs: bo,
        internal_entities: ie,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeResult {
    pub merged_source: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub linked_entities: Vec<String>,
}

pub fn compose_systems(source_a: &str, source_b: &str, links_str: &str) -> ComposeResult {
    let mut links: HashMap<String, String> = HashMap::new();
    let trimmed = links_str.trim();
    if !trimmed.is_empty() {
        for pair in trimmed.split(',') {
            let parts: Vec<&str> = pair.split('=').collect();
            if parts.len() == 2 {
                let left = parts[0].trim();
                let right = parts[1].trim();
                let ln = if left.starts_with("a.") || left.starts_with("b.") {
                    &left[2..]
                } else {
                    left
                };
                let rn = if right.starts_with("a.") || right.starts_with("b.") {
                    &right[2..]
                } else {
                    right
                };
                links.insert(ln.to_lowercase(), rn.to_string());
            }
        }
    }
    let aa = parse_en(source_a);
    let ab = parse_en(source_b);
    let rb: Vec<ENAction> = ab
        .iter()
        .map(|a| ENAction {
            subject: links
                .get(&a.subject.to_lowercase())
                .cloned()
                .unwrap_or_else(|| a.subject.clone()),
            action: a.action.clone(),
            needs: a
                .needs
                .iter()
                .map(|n| {
                    links
                        .get(&n.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| n.clone())
                })
                .collect(),
            yields: a
                .yields
                .iter()
                .map(|y| {
                    links
                        .get(&y.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| y.clone())
                })
                .collect(),
            subsystem: a.subsystem.clone(),
        })
        .collect();
    let mut all = aa;
    all.extend(rb);
    let ms: String = all
        .iter()
        .map(|a| {
            let mut l = format!("{} do: {}", a.subject, a.action);
            if !a.needs.is_empty() {
                l.push_str(&format!(" needs: {}", a.needs.join(", ")));
            }
            if !a.yields.is_empty() {
                l.push_str(&format!(" yields: {}", a.yields.join(", ")));
            }
            l
        })
        .collect::<Vec<_>>()
        .join("\n");
    let graph = build_graph(&all);
    ComposeResult {
        merged_source: ms,
        node_count: graph.order(),
        edge_count: graph.size(),
        linked_entities: links.into_values().collect(),
    }
}

pub struct MemoryRecord {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: Option<String>,
}
pub struct LinkRecord {
    pub source_id: i64,
    pub target_id: i64,
    pub link_type: String,
    pub similarity: f64,
}

pub fn analyze_memory_graph(memories: &[MemoryRecord], links: &[LinkRecord]) -> AnalysisResult {
    let mut graph = StructuralGraph::new();
    for mem in memories {
        let id = format!("m{}", mem.id);
        if !graph.has_node(&id) {
            let label = if mem.content.len() > 60 {
                mem.content[..60].to_string()
            } else {
                mem.content.clone()
            };
            graph.add_node(
                &id,
                StructuralNode {
                    id: id.clone(),
                    node_type: "action".to_string(),
                    label,
                    subject: Some(mem.category.clone()),
                    subsystem: None,
                },
            );
        }
    }
    for link in links {
        let sid = format!("m{}", link.source_id);
        let tid = format!("m{}", link.target_id);
        if graph.has_node(&sid) && graph.has_node(&tid) && !graph.has_directed_edge(&sid, &tid) {
            graph.add_directed_edge(&sid, &tid);
        }
    }
    let nodes = classify_node_roles(&graph);
    let bridges = find_bridges(&graph);
    let topology = classify_topology(&graph);
    let components = connected_components(&graph);
    AnalysisResult {
        topology,
        node_count: graph.order(),
        edge_count: graph.size(),
        sources: nodes
            .iter()
            .filter(|n| n.role == NodeRole::SOURCE)
            .map(|n| n.label.clone())
            .collect(),
        sinks: nodes
            .iter()
            .filter(|n| n.role == NodeRole::SINK)
            .map(|n| n.label.clone())
            .collect(),
        hubs: nodes
            .iter()
            .filter(|n| n.role == NodeRole::HUB)
            .map(|n| n.label.clone())
            .collect(),
        nodes,
        bridges,
        components: components.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_en_basic() {
        let source =
            "Fetcher do: fetch needs: URL yields: HTML\nParser do: parse needs: HTML yields: AST";
        let actions = parse_en(source);
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].subject, "Fetcher");
        assert_eq!(actions[0].needs, vec!["URL"]);
        assert_eq!(actions[0].yields, vec!["HTML"]);
    }

    #[test]
    fn test_parse_en_subsystem() {
        let source = "[Frontend]\nFetcher do: fetch yields: HTML\n[Backend]\nServer do: serve needs: Request yields: Response";
        let actions = parse_en(source);
        assert_eq!(actions[0].subsystem, Some("Frontend".to_string()));
        assert_eq!(actions[1].subsystem, Some("Backend".to_string()));
    }

    #[test]
    fn test_entity_id() {
        assert_eq!(entity_id("Hello World"), "hello_world");
        assert_eq!(entity_id("MyThing"), "mything");
    }

    #[test]
    fn test_classify_topology_pipeline() {
        let source = "A do: s1 yields: X\nB do: s2 needs: X yields: Y\nC do: s3 needs: Y yields: Z";
        assert_eq!(
            classify_topology(&build_graph(&parse_en(source))),
            TopologyType::Pipeline
        );
    }

    #[test]
    fn test_classify_topology_fork_join() {
        let source = "Splitter do: split yields: A, B\nWA do: a needs: A yields: RA\nWB do: b needs: B yields: RB\nJoiner do: merge needs: RA, RB yields: Out";
        assert_eq!(
            classify_topology(&build_graph(&parse_en(source))),
            TopologyType::ForkJoin
        );
    }

    #[test]
    fn test_node_roles() {
        let source = "A do: s1 yields: X\nB do: s2 needs: X yields: Y";
        let roles = classify_node_roles(&build_graph(&parse_en(source)));
        assert_eq!(
            roles.iter().find(|r| r.id == "a").unwrap().role,
            NodeRole::SOURCE
        );
        assert_eq!(
            roles.iter().find(|r| r.id == "y").unwrap().role,
            NodeRole::SINK
        );
        assert_eq!(
            roles.iter().find(|r| r.id == "x").unwrap().role,
            NodeRole::PIPELINE
        );
    }

    #[test]
    fn test_find_bridges() {
        let source = "A do: s1 yields: X\nB do: s2 needs: X yields: Y";
        assert!(find_bridges(&build_graph(&parse_en(source))).len() >= 2);
    }

    #[test]
    fn test_analyze_system() {
        let r = analyze_system(
            "Fetcher do: fetch needs: URL yields: HTML\nParser do: parse needs: HTML yields: AST",
        );
        assert_eq!(r.topology, TopologyType::Pipeline);
        assert_eq!(r.node_count, 5);
        assert_eq!(r.edge_count, 4);
    }

    #[test]
    fn test_betweenness() {
        let r = compute_betweenness("Center do: route needs: A, B, C yields: X, Y, Z", "Center");
        assert!(r.centrality >= 0.0);
        assert!(!r.all_centralities.is_empty());
    }

    #[test]
    fn test_distance() {
        let r = compute_distance(
            "A do: s1 yields: X\nB do: s2 needs: X yields: Y\nC do: s3 needs: Y yields: Z",
            "A",
            "Z",
        );
        assert!(r.distance.is_some());
        assert!(r.distance.unwrap() > 0);
    }

    #[test]
    fn test_impact() {
        let r = compute_impact(
            "A do: s1 yields: X\nB do: s2 needs: X yields: Y\nC do: s3 needs: Y yields: Z",
            "B",
        );
        assert_eq!(r.removed_label, "B");
        assert!(r.after_components >= r.original_components);
    }

    #[test]
    fn test_diff() {
        let d = structural_diff(
            "A do: s1 yields: X",
            "A do: s1 yields: X\nB do: s2 needs: X yields: Y",
        );
        assert!(!d.nodes_only_in_b.is_empty());
    }

    #[test]
    fn test_compose() {
        let r = compose_systems(
            "A do: s1 yields: OutputA",
            "B do: s2 needs: InputB yields: RB",
            "b.InputB=a.OutputA",
        );
        assert!(r.node_count > 0);
        assert!(r.linked_entities.contains(&"OutputA".to_string()));
    }

    #[test]
    fn test_louvain() {
        let r = categorize_system("A do: work needs: X yields: Y\nB do: process needs: Y yields: Z\nC do: other needs: P yields: Q");
        assert!(!r.subsystems.is_empty());
    }

    #[test]
    fn test_detail() {
        let r = detail_analysis(
            "A do: s1 yields: X\nB do: s2 needs: X yields: Y\nC do: s3 needs: Y yields: Z",
        );
        assert_eq!(r.topology, TopologyType::Pipeline);
        assert!(!r.critical_path.is_empty());
    }

    #[test]
    fn test_memory_graph() {
        let m = vec![
            MemoryRecord {
                id: 1,
                content: "First".into(),
                category: "general".into(),
                source: None,
            },
            MemoryRecord {
                id: 2,
                content: "Second".into(),
                category: "general".into(),
                source: None,
            },
        ];
        let l = vec![LinkRecord {
            source_id: 1,
            target_id: 2,
            link_type: "related".into(),
            similarity: 0.8,
        }];
        let r = analyze_memory_graph(&m, &l);
        assert_eq!(r.node_count, 2);
        assert_eq!(r.edge_count, 1);
    }
}
