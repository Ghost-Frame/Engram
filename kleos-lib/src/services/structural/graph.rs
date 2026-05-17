//! Directed graph + structural classification primitives used by the
//! analyzer. Nodes are identified by the EN `subject`; edges are formed by
//! matching `yields:` producers to downstream `needs:` consumers.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use super::parser::EnStatement;

/// Topology classification of the parsed graph. Matches the
/// `structural_analyze` documented set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum Topology {
    Pipeline,
    Tree,
    ForkJoin,
    Dag,
    Cycle,
    Disconnected,
    Empty,
}

/// Per-node role tag. Mirrors the documented set
/// `SOURCE / SINK / FORK / JOIN / HUB / PIPELINE`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum NodeRole {
    Source,
    Sink,
    Fork,
    Join,
    Hub,
    Pipeline,
    Isolated,
}

/// A directed graph backed by adjacency lists. Node ids are dense `usize`
/// values; `names[id]` recovers the original subject.
#[derive(Debug, Clone)]
pub struct Graph {
    /// `names[i]` -> subject for node id `i`.
    pub names: Vec<String>,
    /// Lookup by name.
    pub index: HashMap<String, usize>,
    /// `out_edges[i]` -> sorted vec of downstream node ids.
    pub out_edges: Vec<Vec<usize>>,
    /// `in_edges[i]` -> sorted vec of upstream node ids.
    pub in_edges: Vec<Vec<usize>>,
}

impl Graph {
    /// Build a graph from parsed EN statements. Every `subject` becomes a
    /// node; every yielded token paired with a downstream `needs:` of the
    /// same token becomes a directed edge. Tokens that only appear in
    /// `yields:` (no consumer) and tokens that only appear in `needs:`
    /// (no producer) are deliberately ignored -- they are not nodes.
    pub fn from_statements(stmts: &[EnStatement]) -> Self {
        // 1) Allocate node ids in input order. Stable ordering keeps the
        //    role/role and topology output predictable.
        let mut names: Vec<String> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        for s in stmts {
            if !index.contains_key(&s.subject) {
                index.insert(s.subject.clone(), names.len());
                names.push(s.subject.clone());
            }
        }

        let n = names.len();
        let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut in_edges: Vec<Vec<usize>> = vec![Vec::new(); n];

        // 2) Index producers by yielded token, then for every consumer that
        //    needs that token, draw an edge producer -> consumer.
        let mut producers: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for s in stmts {
            let id = index[&s.subject];
            for y in &s.yields {
                producers.entry(y.as_str()).or_default().push(id);
            }
        }
        for s in stmts {
            let cid = index[&s.subject];
            for n_tok in &s.needs {
                if let Some(prods) = producers.get(n_tok.as_str()) {
                    for &pid in prods {
                        if pid == cid {
                            continue;
                        }
                        if !out_edges[pid].contains(&cid) {
                            out_edges[pid].push(cid);
                        }
                        if !in_edges[cid].contains(&pid) {
                            in_edges[cid].push(pid);
                        }
                    }
                }
            }
        }
        for v in out_edges.iter_mut().chain(in_edges.iter_mut()) {
            v.sort_unstable();
        }

        Graph {
            names,
            index,
            out_edges,
            in_edges,
        }
    }

    /// Node count.
    pub fn node_count(&self) -> usize {
        self.names.len()
    }

    /// Number of directed edges (each producer-consumer pair counted once).
    pub fn edge_count(&self) -> usize {
        self.out_edges.iter().map(|v| v.len()).sum()
    }

    /// Per-node role classification using only in/out degrees.
    pub fn node_role(&self, id: usize) -> NodeRole {
        let in_deg = self.in_edges[id].len();
        let out_deg = self.out_edges[id].len();
        match (in_deg, out_deg) {
            (0, 0) => NodeRole::Isolated,
            (0, _) => NodeRole::Source,
            (_, 0) => NodeRole::Sink,
            (1, 1) => NodeRole::Pipeline,
            (1, m) if m > 1 => NodeRole::Fork,
            (m, 1) if m > 1 => NodeRole::Join,
            _ => NodeRole::Hub,
        }
    }

    /// Returns the connected component ids treating edges as undirected.
    fn undirected_components(&self) -> Vec<usize> {
        let n = self.node_count();
        let mut comp = vec![usize::MAX; n];
        let mut next_id = 0usize;
        for start in 0..n {
            if comp[start] != usize::MAX {
                continue;
            }
            // BFS over undirected adjacency.
            let mut q = VecDeque::new();
            q.push_back(start);
            comp[start] = next_id;
            while let Some(node) = q.pop_front() {
                for &neigh in self.out_edges[node]
                    .iter()
                    .chain(self.in_edges[node].iter())
                {
                    if comp[neigh] == usize::MAX {
                        comp[neigh] = next_id;
                        q.push_back(neigh);
                    }
                }
            }
            next_id += 1;
        }
        comp
    }

    /// Number of weakly connected components.
    pub fn component_count(&self) -> usize {
        if self.node_count() == 0 {
            return 0;
        }
        self.undirected_components()
            .iter()
            .copied()
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }

    /// Returns true when the directed graph has a cycle (Kahn's algorithm
    /// detects this by checking the topological ordering's coverage).
    pub fn has_cycle(&self) -> bool {
        let n = self.node_count();
        let mut indeg: Vec<usize> = self.in_edges.iter().map(|v| v.len()).collect();
        let mut q: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        let mut visited = 0usize;
        while let Some(u) = q.pop_front() {
            visited += 1;
            for &v in &self.out_edges[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    q.push_back(v);
                }
            }
        }
        visited < n
    }

    /// Classify the whole graph topology. Logic:
    /// 1. Empty when no nodes.
    /// 2. Disconnected when more than one weakly connected component.
    /// 3. Cycle when any directed cycle exists.
    /// 4. Pipeline when every node has in_deg <= 1 and out_deg <= 1.
    /// 5. Tree when the graph is a single weakly connected component with
    ///    `edges == nodes - 1` and no node has more than one parent.
    /// 6. ForkJoin when there is at least one FORK and at least one JOIN.
    /// 7. DAG otherwise.
    pub fn classify_topology(&self) -> Topology {
        if self.node_count() == 0 {
            return Topology::Empty;
        }
        if self.component_count() > 1 {
            return Topology::Disconnected;
        }
        if self.has_cycle() {
            return Topology::Cycle;
        }
        let mut has_fork = false;
        let mut has_join = false;
        let mut all_simple = true;
        for i in 0..self.node_count() {
            let in_d = self.in_edges[i].len();
            let out_d = self.out_edges[i].len();
            if out_d > 1 {
                has_fork = true;
            }
            if in_d > 1 {
                has_join = true;
            }
            if in_d > 1 || out_d > 1 {
                all_simple = false;
            }
        }
        if all_simple {
            return Topology::Pipeline;
        }
        if self.edge_count() == self.node_count() - 1 && !has_join {
            return Topology::Tree;
        }
        if has_fork && has_join {
            return Topology::ForkJoin;
        }
        Topology::Dag
    }

    /// Bridges in the underlying undirected graph: edges whose removal
    /// disconnects a component. Implemented with Tarjan's DFS classic.
    /// Returns ordered (u, v) name pairs with u < v lexically.
    pub fn bridges(&self) -> Vec<(String, String)> {
        let n = self.node_count();
        let mut disc = vec![usize::MAX; n];
        let mut low = vec![0usize; n];
        let mut parent = vec![usize::MAX; n];
        let mut timer = 0usize;
        let mut out: BTreeSet<(String, String)> = BTreeSet::new();

        // Undirected adjacency built once.
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for u in 0..n {
            for &v in &self.out_edges[u] {
                if !adj[u].contains(&v) {
                    adj[u].push(v);
                }
                if !adj[v].contains(&u) {
                    adj[v].push(u);
                }
            }
        }

        for start in 0..n {
            if disc[start] != usize::MAX {
                continue;
            }
            // Iterative DFS to avoid deep recursion on long pipelines.
            let mut stack: Vec<(usize, usize)> = Vec::new();
            disc[start] = timer;
            low[start] = timer;
            timer += 1;
            stack.push((start, 0));

            while let Some(&(u, idx)) = stack.last() {
                if idx < adj[u].len() {
                    let v = adj[u][idx];
                    let last = stack.len() - 1;
                    stack[last].1 += 1;
                    if disc[v] == usize::MAX {
                        parent[v] = u;
                        disc[v] = timer;
                        low[v] = timer;
                        timer += 1;
                        stack.push((v, 0));
                    } else if v != parent[u] {
                        low[u] = low[u].min(disc[v]);
                    }
                } else {
                    stack.pop();
                    if let Some(&(parent_u, _)) = stack.last() {
                        low[parent_u] = low[parent_u].min(low[u]);
                        if low[u] > disc[parent_u] {
                            let a = &self.names[parent_u];
                            let b = &self.names[u];
                            let pair = if a <= b {
                                (a.clone(), b.clone())
                            } else {
                                (b.clone(), a.clone())
                            };
                            out.insert(pair);
                        }
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// BFS shortest path between two node ids. Returns the ordered sequence
    /// of names from `src` to `dst` inclusive. Edges are followed
    /// directionally (yields -> needs). Returns `None` when no path exists.
    pub fn bfs_path(&self, src: usize, dst: usize) -> Option<Vec<String>> {
        let n = self.node_count();
        if src >= n || dst >= n {
            return None;
        }
        if src == dst {
            return Some(vec![self.names[src].clone()]);
        }
        let mut prev: Vec<Option<usize>> = vec![None; n];
        let mut seen = vec![false; n];
        let mut q = VecDeque::new();
        seen[src] = true;
        q.push_back(src);
        while let Some(u) = q.pop_front() {
            for &v in &self.out_edges[u] {
                if !seen[v] {
                    seen[v] = true;
                    prev[v] = Some(u);
                    if v == dst {
                        let mut path = vec![self.names[v].clone()];
                        let mut cur = u;
                        loop {
                            path.push(self.names[cur].clone());
                            if cur == src {
                                break;
                            }
                            cur = prev[cur].expect("prev chain");
                        }
                        path.reverse();
                        return Some(path);
                    }
                    q.push_back(v);
                }
            }
        }
        None
    }

    /// Undirected BFS path: same as [`bfs_path`] but ignores edge direction.
    /// Returns the path plus the set of reverse-direction edges traversed
    /// (so callers can flag where the trace flowed against an arrow).
    #[allow(clippy::type_complexity)]
    pub fn bfs_path_undirected(
        &self,
        src: usize,
        dst: usize,
    ) -> Option<(Vec<String>, Vec<(String, String)>)> {
        let n = self.node_count();
        if src >= n || dst >= n {
            return None;
        }
        if src == dst {
            return Some((vec![self.names[src].clone()], Vec::new()));
        }
        let mut prev: Vec<Option<usize>> = vec![None; n];
        let mut seen = vec![false; n];
        let mut q = VecDeque::new();
        seen[src] = true;
        q.push_back(src);
        let directed: BTreeSet<(usize, usize)> = (0..n)
            .flat_map(|u| self.out_edges[u].iter().map(move |&v| (u, v)))
            .collect();

        while let Some(u) = q.pop_front() {
            for &v in self.out_edges[u].iter().chain(self.in_edges[u].iter()) {
                if !seen[v] {
                    seen[v] = true;
                    prev[v] = Some(u);
                    if v == dst {
                        let mut ids = vec![v];
                        let mut cur = u;
                        loop {
                            ids.push(cur);
                            if cur == src {
                                break;
                            }
                            cur = prev[cur].expect("prev chain");
                        }
                        ids.reverse();
                        let mut path: Vec<String> = Vec::with_capacity(ids.len());
                        let mut reverse_edges: Vec<(String, String)> = Vec::new();
                        for win in ids.windows(2) {
                            let (a, b) = (win[0], win[1]);
                            if !directed.contains(&(a, b)) && directed.contains(&(b, a)) {
                                reverse_edges.push((self.names[a].clone(), self.names[b].clone()));
                            }
                        }
                        for id in &ids {
                            path.push(self.names[*id].clone());
                        }
                        return Some((path, reverse_edges));
                    }
                    q.push_back(v);
                }
            }
        }
        None
    }

    /// Brandes' betweenness centrality, normalised to `[0, 1]` by dividing
    /// by `(n-1)(n-2)` for directed graphs (or 0 when fewer than 3 nodes).
    /// Returns a map from name to its normalised score.
    pub fn betweenness(&self) -> BTreeMap<String, f64> {
        let n = self.node_count();
        let mut cb: Vec<f64> = vec![0.0; n];
        for s in 0..n {
            let mut stack: Vec<usize> = Vec::new();
            let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
            let mut sigma: Vec<f64> = vec![0.0; n];
            let mut dist: Vec<i64> = vec![-1; n];
            sigma[s] = 1.0;
            dist[s] = 0;
            let mut q = VecDeque::new();
            q.push_back(s);
            while let Some(v) = q.pop_front() {
                stack.push(v);
                for &w in &self.out_edges[v] {
                    if dist[w] < 0 {
                        dist[w] = dist[v] + 1;
                        q.push_back(w);
                    }
                    if dist[w] == dist[v] + 1 {
                        sigma[w] += sigma[v];
                        preds[w].push(v);
                    }
                }
            }
            let mut delta: Vec<f64> = vec![0.0; n];
            while let Some(w) = stack.pop() {
                for &v in &preds[w] {
                    delta[v] += (sigma[v] / sigma[w]) * (1.0 + delta[w]);
                }
                if w != s {
                    cb[w] += delta[w];
                }
            }
        }
        let denom = if n > 2 {
            ((n - 1) * (n - 2)) as f64
        } else {
            1.0
        };
        let mut out = BTreeMap::new();
        for (i, raw) in cb.iter().enumerate().take(n) {
            let score = if denom > 0.0 { raw / denom } else { 0.0 };
            out.insert(self.names[i].clone(), score);
        }
        out
    }

    /// Critical path length (longest directed path) using DAG DP. Cyclic
    /// graphs return `None`.
    pub fn critical_path_length(&self) -> Option<usize> {
        if self.has_cycle() {
            return None;
        }
        let n = self.node_count();
        // Topological order via Kahn's.
        let mut order: Vec<usize> = Vec::with_capacity(n);
        let mut indeg: Vec<usize> = self.in_edges.iter().map(|v| v.len()).collect();
        let mut q: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        while let Some(u) = q.pop_front() {
            order.push(u);
            for &v in &self.out_edges[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    q.push_back(v);
                }
            }
        }
        let mut depth: Vec<usize> = vec![0; n];
        for u in order {
            for &v in &self.out_edges[u] {
                if depth[v] < depth[u] + 1 {
                    depth[v] = depth[u] + 1;
                }
            }
        }
        depth.into_iter().max()
    }

    /// Flow depth per node: longest distance from any source. None if cyclic.
    pub fn flow_depth(&self) -> Option<BTreeMap<String, usize>> {
        if self.has_cycle() {
            return None;
        }
        let n = self.node_count();
        let mut indeg: Vec<usize> = self.in_edges.iter().map(|v| v.len()).collect();
        let mut q: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
        let mut depth: Vec<usize> = vec![0; n];
        let mut order: Vec<usize> = Vec::with_capacity(n);
        while let Some(u) = q.pop_front() {
            order.push(u);
            for &v in &self.out_edges[u] {
                indeg[v] -= 1;
                if indeg[v] == 0 {
                    q.push_back(v);
                }
            }
        }
        for u in order {
            for &v in &self.out_edges[u] {
                if depth[v] < depth[u] + 1 {
                    depth[v] = depth[u] + 1;
                }
            }
        }
        let mut out = BTreeMap::new();
        for (i, d) in depth.iter().enumerate().take(n) {
            out.insert(self.names[i].clone(), *d);
        }
        Some(out)
    }

    /// Concurrency width per depth level: number of nodes that sit at the
    /// same flow_depth. Returns None on cyclic graphs.
    pub fn concurrency_by_level(&self) -> Option<BTreeMap<usize, usize>> {
        let depths = self.flow_depth()?;
        let mut levels: BTreeMap<usize, usize> = BTreeMap::new();
        for d in depths.values() {
            *levels.entry(*d).or_insert(0) += 1;
        }
        Some(levels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::structural::parser::parse_en_source;

    fn graph(src: &str) -> Graph {
        Graph::from_statements(&parse_en_source(src))
    }

    #[test]
    fn pipeline_is_classified_correctly() {
        let g = graph("A yields: x. B needs: x yields: y. C needs: y.");
        assert_eq!(g.classify_topology(), Topology::Pipeline);
        assert_eq!(g.node_count(), 3);
    }

    #[test]
    fn fork_join_topology() {
        let g = graph(
            "Root yields: a. \
             L needs: a yields: x. R needs: a yields: y. \
             Sink needs: x. Sink needs: y.",
        );
        assert_eq!(g.classify_topology(), Topology::ForkJoin);
    }

    #[test]
    fn cycle_detected() {
        let g = graph("A needs: x yields: y. B needs: y yields: x.");
        assert_eq!(g.classify_topology(), Topology::Cycle);
        assert!(g.has_cycle());
    }

    #[test]
    fn disconnected_components() {
        let g = graph("A yields: x. B needs: x. C yields: q. D needs: q.");
        assert_eq!(g.classify_topology(), Topology::Disconnected);
        assert_eq!(g.component_count(), 2);
    }

    #[test]
    fn bridges_in_pipeline() {
        let g = graph("A yields: x. B needs: x yields: y. C needs: y.");
        // Every edge is a bridge in a pipeline.
        let bridges = g.bridges();
        assert_eq!(bridges.len(), 2);
    }

    #[test]
    fn betweenness_intermediate_node_is_highest() {
        let g = graph("A yields: x. B needs: x yields: y. C needs: y.");
        let bt = g.betweenness();
        assert!(bt["B"] > bt["A"]);
        assert!(bt["B"] > bt["C"]);
    }

    #[test]
    fn bfs_path_simple() {
        let g = graph("A yields: x. B needs: x yields: y. C needs: y.");
        let a = g.index["A"];
        let c = g.index["C"];
        let p = g.bfs_path(a, c).unwrap();
        assert_eq!(p, vec!["A".to_string(), "B".to_string(), "C".to_string()]);
    }

    #[test]
    fn critical_path_is_pipeline_length() {
        let g = graph("A yields: x. B needs: x yields: y. C needs: y.");
        assert_eq!(g.critical_path_length(), Some(2));
    }
}
