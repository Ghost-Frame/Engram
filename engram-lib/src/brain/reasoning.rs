//! Neural reasoning engine -- substrate-native inference generation.
//!
//! Ported from Eidolon's reasoning module. Operates on BrainPattern slices
//! and BrainEdge slices (loaded from the DB) instead of an in-memory
//! ConnectionGraph. Uses Vec<f32> throughout instead of ndarray.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use crate::brain::hopfield::edges::{BrainEdge, EdgeType};
use crate::brain::hopfield::network::HopfieldNetwork;
use crate::brain::hopfield::pattern::BrainPattern;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// The kind of inference produced by the reasoning engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceKind {
    Abductive,
    Predictive,
    Synthesis,
    Rule,
    Analogical,
}

/// A single inference produced by the reasoning engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Inference {
    pub kind: InferenceKind,
    pub description: String,
    pub confidence: f32,
    pub supporting_ids: Vec<i64>,
}

/// Configuration controlling which reasoning modes are active and their
/// thresholds.
#[derive(Debug, Clone)]
pub struct ReasoningConfig {
    pub enabled: bool,
    pub abductive: bool,
    pub predictive: bool,
    pub synthesis: bool,
    pub rule_extraction: bool,
    pub analogical: bool,
    pub max_inferences: usize,
    pub min_confidence: f32,
}

impl Default for ReasoningConfig {
    fn default() -> Self {
        ReasoningConfig {
            enabled: true,
            abductive: true,
            predictive: true,
            synthesis: true,
            rule_extraction: true,
            analogical: false, // Most expensive, disabled by default
            max_inferences: 5,
            min_confidence: 0.3,
        }
    }
}

/// A contradiction pair: two patterns whose content conflicts.
/// winner_id is the currently stronger/more-activated pattern.
#[derive(Debug, Clone)]
pub struct ContradictionPair {
    pub winner_id: i64,
    pub loser_id: i64,
    pub winner_activation: f32,
    pub loser_activation: f32,
}

// ---------------------------------------------------------------------------
// Adjacency helpers (replaces ConnectionGraph)
// ---------------------------------------------------------------------------

/// Build a forward adjacency map (source -> [(target, weight, edge_type)])
/// from a flat edge slice. Scoped to one edge type for directional traversal.
fn build_forward_adj(edges: &[BrainEdge], kind: EdgeType) -> HashMap<i64, Vec<(i64, f32)>> {
    let mut adj: HashMap<i64, Vec<(i64, f32)>> = HashMap::new();
    for e in edges {
        if e.edge_type == kind {
            adj.entry(e.source_id).or_default().push((e.target_id, e.weight));
        }
    }
    adj
}

/// Build a backward adjacency map (target -> [(source, weight)]) for
/// predecessor traversal (used by abductive reasoning).
fn build_backward_adj(edges: &[BrainEdge], kind: EdgeType) -> HashMap<i64, Vec<(i64, f32)>> {
    let mut adj: HashMap<i64, Vec<(i64, f32)>> = HashMap::new();
    for e in edges {
        if e.edge_type == kind {
            adj.entry(e.target_id).or_default().push((e.source_id, e.weight));
        }
    }
    adj
}

/// Build a full adjacency map over all edge types for structural analysis.
/// Maps node_id -> [(target_id, weight, edge_type_str)].
fn build_full_adj(edges: &[BrainEdge]) -> HashMap<i64, Vec<(i64, f32, String)>> {
    let mut adj: HashMap<i64, Vec<(i64, f32, String)>> = HashMap::new();
    for e in edges {
        adj.entry(e.source_id)
            .or_default()
            .push((e.target_id, e.weight, e.edge_type.to_string()));
    }
    adj
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn content_preview(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        content.to_string()
    } else {
        let boundary = content
            .char_indices()
            .nth(max_len)
            .map(|(i, _)| i)
            .unwrap_or(content.len());
        format!("{}...", &content[..boundary])
    }
}

fn build_chain_description(
    chain: &[i64],
    content_map: &HashMap<i64, String>,
    verb: &str,
) -> String {
    let previews: Vec<String> = chain
        .iter()
        .filter_map(|id| content_map.get(id).map(|c| content_preview(c, 60)))
        .collect();
    if previews.len() <= 1 {
        return previews.into_iter().next().unwrap_or_default();
    }
    let root = previews.last().unwrap();
    let effect = previews.first().unwrap();
    if previews.len() == 2 {
        format!("'{}' {} '{}'", root, verb, effect)
    } else {
        let intermediates: Vec<&str> = previews[1..previews.len() - 1]
            .iter()
            .map(|s| s.as_str())
            .collect();
        format!("'{}' -> [{}] -> '{}'", root, intermediates.join(" -> "), effect)
    }
}

fn build_prediction_description(chain: &[i64], content_map: &HashMap<i64, String>) -> String {
    let previews: Vec<String> = chain
        .iter()
        .filter_map(|id| content_map.get(id).map(|c| content_preview(c, 60)))
        .collect();
    if previews.len() <= 1 {
        return previews.into_iter().next().unwrap_or_default();
    }
    let situation = previews.first().unwrap();
    let consequence = previews.last().unwrap();
    if previews.len() == 2 {
        format!("If '{}', then '{}'", situation, consequence)
    } else {
        let intermediates: Vec<&str> = previews[1..previews.len() - 1]
            .iter()
            .map(|s| s.as_str())
            .collect();
        format!(
            "If '{}', then '{}' (via {})",
            situation,
            consequence,
            intermediates.join(", ")
        )
    }
}

/// Compute a structural signature for a set of nodes: set of
/// (edge_type, degree_bucket) pairs, used for analogy matching.
fn subgraph_signature(
    full_adj: &HashMap<i64, Vec<(i64, f32, String)>>,
    nodes: &HashSet<i64>,
) -> HashSet<String> {
    let mut sig: HashSet<String> = HashSet::new();
    for &node in nodes {
        if let Some(edges) = full_adj.get(&node) {
            let degree = edges.len();
            for (_, _, et) in edges {
                sig.insert(format!("{}:{}", et, degree.min(10)));
            }
        }
    }
    sig
}

/// Jaccard similarity between two string sets.
fn jaccard_similarity(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        return 0.0;
    }
    intersection as f32 / union as f32
}

/// Cosine similarity between two Vec<f32> slices.
fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

// ---------------------------------------------------------------------------
// Mode 1: Abductive Reasoning (Backward Causal)
// ---------------------------------------------------------------------------

/// "Why did X happen?" -- traverse Causal edges backward.
///
/// For each strongly-activated pattern, walks backward along Causal edges
/// to find root causes. Merges chains sharing the same root and scores by
/// combined confidence weighted by the root's pattern strength.
pub fn abductive_reason(
    edges: &[BrainEdge],
    patterns: &[BrainPattern],
    memory_index: &HashMap<i64, usize>,
    content_map: &HashMap<i64, String>,
    activated: &HashMap<i64, f32>,
    config: &ReasoningConfig,
) -> Vec<Inference> {
    if !config.abductive {
        return vec![];
    }
    let max_depth = 3;
    let backward_adj = build_backward_adj(edges, EdgeType::Causal);
    let mut inferences: Vec<Inference> = Vec::new();

    for (&mem_id, &activation) in activated {
        if activation < 0.5 {
            continue;
        }

        // Backward BFS on Causal edges
        let mut queue: VecDeque<(i64, Vec<i64>, f32)> = VecDeque::new();
        queue.push_back((mem_id, vec![mem_id], 1.0));
        let mut visited: HashSet<i64> = HashSet::new();
        visited.insert(mem_id);
        let mut chains: Vec<(Vec<i64>, f32)> = Vec::new();

        while let Some((current, path, confidence)) = queue.pop_front() {
            if path.len() > max_depth + 1 {
                continue;
            }
            if let Some(predecessors) = backward_adj.get(&current) {
                for &(pred_id, weight) in predecessors {
                    if visited.contains(&pred_id) {
                        continue;
                    }
                    visited.insert(pred_id);
                    let chain_conf = confidence * weight;
                    if chain_conf < config.min_confidence {
                        continue;
                    }
                    let mut new_path = path.clone();
                    new_path.push(pred_id);
                    chains.push((new_path.clone(), chain_conf));
                    if new_path.len() <= max_depth + 1 {
                        queue.push_back((pred_id, new_path, chain_conf));
                    }
                }
            }
        }

        // Merge chains sharing same root cause
        let mut root_groups: HashMap<i64, Vec<f32>> = HashMap::new();
        let mut root_chains: HashMap<i64, Vec<i64>> = HashMap::new();
        for (chain, conf) in &chains {
            let root = *chain.last().unwrap_or(&mem_id);
            root_groups.entry(root).or_default().push(*conf);
            root_chains.entry(root).or_insert_with(|| chain.clone());
        }

        for (root_id, confidences) in root_groups {
            let combined =
                1.0 - confidences
                    .iter()
                    .fold(1.0_f32, |acc, c| acc * (1.0 - c));
            if combined < config.min_confidence {
                continue;
            }

            let chain = root_chains.get(&root_id).cloned().unwrap_or_default();
            let description = build_chain_description(&chain, content_map, "caused");

            // Apply root pattern's strength (analogous to decay_factor)
            let root_strength = memory_index
                .get(&root_id)
                .map(|&idx| patterns[idx].strength)
                .unwrap_or(1.0);

            // Collect all IDs in the chain as supporting_ids
            let supporting_ids = chain.clone();

            inferences.push(Inference {
                kind: InferenceKind::Abductive,
                description,
                confidence: combined * root_strength,
                supporting_ids,
            });
        }
    }

    inferences.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    inferences.truncate(config.max_inferences);
    inferences
}

// ---------------------------------------------------------------------------
// Mode 2: Predictive Reasoning (Forward Causal)
// ---------------------------------------------------------------------------

/// "What will X cause?" -- traverse Causal edges forward.
///
/// For each strongly-activated pattern, walks forward along Causal edges
/// to predict downstream consequences not already in the activated set.
pub fn predictive_reason(
    edges: &[BrainEdge],
    patterns: &[BrainPattern],
    memory_index: &HashMap<i64, usize>,
    content_map: &HashMap<i64, String>,
    activated: &HashMap<i64, f32>,
    config: &ReasoningConfig,
) -> Vec<Inference> {
    if !config.predictive {
        return vec![];
    }
    let max_depth = 3;
    let spread_decay: f32 = 0.5;
    let forward_adj = build_forward_adj(edges, EdgeType::Causal);
    let mut inferences: Vec<Inference> = Vec::new();
    let activated_set: HashSet<i64> = activated.keys().cloned().collect();

    // Suppress unused warning: memory_index and patterns are available for
    // future per-pattern strength weighting (consistent with abductive).
    let _ = (patterns, memory_index);

    for (&mem_id, &activation) in activated {
        if activation < 0.5 {
            continue;
        }

        // Forward BFS on Causal edges
        let mut queue: VecDeque<(i64, Vec<i64>, f32, usize)> = VecDeque::new();
        queue.push_back((mem_id, vec![mem_id], 1.0, 0));
        let mut visited: HashSet<i64> = HashSet::new();
        visited.insert(mem_id);

        while let Some((current, path, confidence, hops)) = queue.pop_front() {
            if hops >= max_depth {
                continue;
            }
            if let Some(successors) = forward_adj.get(&current) {
                for &(succ_id, weight) in successors {
                    if visited.contains(&succ_id) {
                        continue;
                    }
                    visited.insert(succ_id);
                    let chain_conf =
                        confidence * weight * spread_decay.powi((hops + 1) as i32);
                    if chain_conf < config.min_confidence {
                        continue;
                    }
                    let mut new_path = path.clone();
                    new_path.push(succ_id);

                    // Only emit predictions for consequences not already activated
                    if !activated_set.contains(&succ_id) {
                        let description =
                            build_prediction_description(&new_path, content_map);
                        let supporting_ids = new_path.clone();

                        inferences.push(Inference {
                            kind: InferenceKind::Predictive,
                            description,
                            confidence: chain_conf,
                            supporting_ids,
                        });
                    }

                    if hops + 1 < max_depth {
                        queue.push_back((succ_id, new_path, confidence * weight, hops + 1));
                    }
                }
            }
        }
    }

    inferences.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    inferences.truncate(config.max_inferences);
    inferences
}

// ---------------------------------------------------------------------------
// Mode 3: Contradiction Synthesis
// ---------------------------------------------------------------------------

/// Produce temporal understanding from contradiction pairs.
///
/// Each pair yields a Synthesis inference describing how the winner belief
/// superseded the loser belief, ordered by their creation timestamps.
pub fn synthesize_contradictions(
    contradictions: &[ContradictionPair],
    patterns: &[BrainPattern],
    memory_index: &HashMap<i64, usize>,
    content_map: &HashMap<i64, String>,
    config: &ReasoningConfig,
) -> Vec<Inference> {
    if !config.synthesis {
        return vec![];
    }
    let mut inferences: Vec<Inference> = Vec::new();

    for pair in contradictions {
        let winner_idx = match memory_index.get(&pair.winner_id) {
            Some(&idx) => idx,
            None => continue,
        };
        let loser_idx = match memory_index.get(&pair.loser_id) {
            Some(&idx) => idx,
            None => continue,
        };

        let winner = &patterns[winner_idx];
        let loser = &patterns[loser_idx];

        let winner_preview = content_map
            .get(&winner.id)
            .map(|c| content_preview(c, 80))
            .unwrap_or_else(|| format!("pattern {}", winner.id));
        let loser_preview = content_map
            .get(&loser.id)
            .map(|c| content_preview(c, 80))
            .unwrap_or_else(|| format!("pattern {}", loser.id));

        let description = if loser.created_at < winner.created_at {
            format!(
                "'{}' was the case until {}, superseded by: '{}'",
                loser_preview, winner.created_at, winner_preview
            )
        } else {
            format!(
                "'{}' replaced earlier understanding: '{}'",
                winner_preview, loser_preview
            )
        };

        let confidence =
            (pair.winner_activation - pair.loser_activation).abs().min(1.0);
        if confidence < config.min_confidence {
            continue;
        }

        inferences.push(Inference {
            kind: InferenceKind::Synthesis,
            description,
            confidence,
            supporting_ids: vec![pair.winner_id, pair.loser_id],
        });
    }

    inferences.truncate(config.max_inferences);
    inferences
}

// ---------------------------------------------------------------------------
// Mode 4: Rule Extraction (Dream-Phase)
// ---------------------------------------------------------------------------

/// Extract implicit rules from strongly co-activated memory clusters.
///
/// Scans all nodes for bidirectional cliques (3+ members connected by
/// strong edges). Each clique is summarised as a cross-category or
/// same-category recurring pattern. Intended to be called during the
/// dream cycle and cached on the brain state.
pub fn extract_rules(
    edges: &[BrainEdge],
    patterns: &[BrainPattern],
    memory_index: &HashMap<i64, usize>,
    content_map: &HashMap<i64, String>,
    category_map: &HashMap<i64, String>,
    min_edge_weight: f32,
) -> Vec<Inference> {
    let full_adj = build_full_adj(edges);
    let mut inferences: Vec<Inference> = Vec::new();

    // For each node, find strong outgoing neighbors
    for (&node_id, adj_edges) in &full_adj {
        let strong_neighbors: Vec<(i64, f32)> = adj_edges
            .iter()
            .filter(|(_, w, _)| *w >= min_edge_weight)
            .map(|(t, w, _)| (*t, *w))
            .collect();

        if strong_neighbors.len() < 2 {
            continue;
        }

        // Detect clique: neighbors that also strongly connect back
        let mut clique_ids: Vec<i64> = vec![node_id];
        for &(neighbor_id, _) in &strong_neighbors {
            let has_reverse = full_adj
                .get(&neighbor_id)
                .map(|e| {
                    e.iter()
                        .any(|(t, w, _)| *t == node_id && *w >= min_edge_weight)
                })
                .unwrap_or(false);
            if has_reverse {
                clique_ids.push(neighbor_id);
            }
        }

        if clique_ids.len() < 3 {
            continue;
        }

        // Collect categories and compute mean edge weight within clique
        let mut categories: HashMap<String, Vec<String>> = HashMap::new();
        let mut mean_weight: f32 = 0.0;
        let mut weight_count: usize = 0;

        for &cid in &clique_ids {
            if memory_index.contains_key(&cid) {
                let cat = category_map
                    .get(&cid)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                let preview = content_map
                    .get(&cid)
                    .map(|c| content_preview(c, 60))
                    .unwrap_or_else(|| format!("pattern {}", cid));
                categories.entry(cat).or_default().push(preview);
            }
            // Sum weights of edges connecting clique members
            for &(neighbor, _) in &strong_neighbors {
                if clique_ids.contains(&neighbor) {
                    if let Some(adj_list) = full_adj.get(&cid) {
                        if let Some((_, w, _)) = adj_list.iter().find(|(t, _, _)| *t == neighbor) {
                            mean_weight += w;
                            weight_count += 1;
                        }
                    }
                }
            }
        }

        if weight_count > 0 {
            mean_weight /= weight_count as f32;
        }

        let description = if categories.len() > 1 {
            let cats: Vec<String> = categories
                .iter()
                .map(|(cat, summaries)| format!("[{}]: {}", cat, summaries.join("; ")))
                .collect();
            format!("Cross-category pattern: {}", cats.join(" <-> "))
        } else {
            let (cat, summaries) = categories.iter().next().unwrap();
            format!("[{}] recurring pattern: {}", cat, summaries.join("; "))
        };

        let confidence = mean_weight * (clique_ids.len() as f32).sqrt()
            / (patterns.len() as f32).sqrt().max(1.0);
        let confidence = confidence.min(1.0);

        let supporting_ids = clique_ids.clone();
        inferences.push(Inference {
            kind: InferenceKind::Rule,
            description,
            confidence,
            supporting_ids,
        });
    }

    inferences.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    inferences.truncate(10); // Cache up to 10 rules
    inferences
}

/// Filter cached rules to those relevant to the current activation set.
pub fn filter_cached_rules(
    cached_rules: &[Inference],
    activated: &HashMap<i64, f32>,
    config: &ReasoningConfig,
) -> Vec<Inference> {
    if !config.rule_extraction {
        return vec![];
    }
    let activated_set: HashSet<i64> = activated.keys().cloned().collect();
    let mut relevant: Vec<Inference> = cached_rules
        .iter()
        .filter(|rule| rule.supporting_ids.iter().any(|id| activated_set.contains(id)))
        .cloned()
        .collect();
    relevant.truncate(config.max_inferences);
    relevant
}

// ---------------------------------------------------------------------------
// Mode 5: Analogical Reasoning (Structural Pattern Matching)
// ---------------------------------------------------------------------------

/// Find structural parallels between memory clusters.
///
/// Uses Hopfield completion to project the query pattern into the stored
/// attractor landscape, then finds non-activated patterns that are both
/// semantically similar to the completed query AND structurally similar
/// (same edge-type / degree fingerprint) to the activated subgraph.
pub fn analogical_reason(
    edges: &[BrainEdge],
    network: &HopfieldNetwork,
    patterns: &[BrainPattern],
    memory_index: &HashMap<i64, usize>,
    content_map: &HashMap<i64, String>,
    activated: &HashMap<i64, f32>,
    query_pattern: &[f32],
    config: &ReasoningConfig,
) -> Vec<Inference> {
    if !config.analogical {
        return vec![];
    }
    let full_adj = build_full_adj(edges);
    let mut inferences: Vec<Inference> = Vec::new();
    let activated_set: HashSet<i64> = activated.keys().cloned().collect();

    // 1. Extract activated subgraph signature
    let activated_signature = subgraph_signature(&full_adj, &activated_set);

    // 2. Pattern completion via Hopfield
    let completed = network.complete(query_pattern, 3, 8.0);

    // 3. Find non-activated patterns similar to the completed pattern
    for mem in patterns {
        if activated_set.contains(&mem.id) {
            continue;
        }
        if mem.pattern.is_empty() {
            continue;
        }
        let sim = cosine_sim(&mem.pattern, &completed);
        if sim < 0.3 {
            continue;
        }

        // 4. Check structural similarity of neighborhoods
        let mem_neighbors: HashSet<i64> = full_adj
            .get(&mem.id)
            .map(|e| e.iter().map(|(t, _, _)| *t).collect())
            .unwrap_or_default();

        let mem_signature = subgraph_signature(&full_adj, &mem_neighbors);
        let structural_sim = jaccard_similarity(&activated_signature, &mem_signature);

        if structural_sim < 0.6 {
            continue;
        }

        let confidence = structural_sim * sim;
        if confidence < config.min_confidence.max(0.5) {
            continue;
        }

        let mem_preview = content_map
            .get(&mem.id)
            .map(|c| content_preview(c, 80))
            .unwrap_or_else(|| format!("pattern {}", mem.id));

        // Find the most-activated pattern as anchor for the analogy label
        let anchor_preview = activated
            .iter()
            .max_by(|a, b| {
                a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)
            })
            .and_then(|(id, _)| memory_index.get(id))
            .and_then(|&idx| content_map.get(&patterns[idx].id))
            .map(|c| content_preview(c, 80))
            .unwrap_or_else(|| "current context".to_string());

        inferences.push(Inference {
            kind: InferenceKind::Analogical,
            description: format!(
                "By analogy with '{}', consider: '{}'",
                anchor_preview, mem_preview
            ),
            confidence,
            supporting_ids: vec![mem.id],
        });

        if inferences.len() >= config.max_inferences {
            break;
        }
    }

    inferences.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    inferences.truncate(config.max_inferences);
    inferences
}
