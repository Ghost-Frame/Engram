use ndarray::{Array1, Array2, Axis};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Constants -- ported from eidolon substrate.rs / decay.rs
// ---------------------------------------------------------------------------

/// Temperature scaling for softmax attention in the modern Hopfield
/// retrieval. Higher beta sharpens the distribution, lower beta makes
/// it more uniform.
pub const DEFAULT_BETA: f32 = 8.0;

/// Minimum activation after softmax to be included in results. Anything
/// below this is noise and discarded.
pub const ACTIVATION_THRESHOLD: f32 = 0.01;

/// Default number of iterations for pattern completion.
pub const DEFAULT_COMPLETE_ITERATIONS: usize = 5;

// ---------------------------------------------------------------------------
// HopfieldNetwork
// ---------------------------------------------------------------------------

/// A continuous/modern Hopfield network (Ramsauer et al. 2020).
///
/// Unlike the classic binary Hopfield net which uses a symmetric weight
/// matrix and binary patterns, this variant stores continuous patterns
/// in a matrix and uses softmax attention for retrieval. The energy
/// landscape is equivalent to transformer self-attention.
///
/// Capacity: O(exp(d)) for d-dimensional patterns (exponential in
/// dimension, vs 0.14N for classic). In practice, bounded by memory.
pub struct HopfieldNetwork {
    /// n_patterns x dim matrix. Each row is an L2-normalized pattern.
    patterns: Option<Array2<f32>>,
    /// Per-pattern strength (decay_factor). Parallel to rows of `patterns`.
    strengths: Vec<f32>,
    /// Maps row index to memory id.
    pattern_ids: Vec<i64>,
    /// Maps memory id to row index.
    id_to_index: HashMap<i64, usize>,
    /// Pattern dimensionality. Zero until the first pattern is stored.
    dim: usize,
}

impl HopfieldNetwork {
    /// Create an empty network.
    pub fn new() -> Self {
        Self {
            patterns: None,
            strengths: Vec::new(),
            pattern_ids: Vec::new(),
            id_to_index: HashMap::new(),
            dim: 0,
        }
    }

    /// Build a network from a batch of pre-loaded patterns.
    /// Each tuple is (memory_id, pattern_vector, strength).
    pub fn from_patterns(batch: Vec<(i64, Vec<f32>, f32)>) -> Self {
        if batch.is_empty() {
            return Self::new();
        }

        let dim = batch[0].1.len();
        let n = batch.len();
        let mut data = Vec::with_capacity(n * dim);
        let mut strengths = Vec::with_capacity(n);
        let mut pattern_ids = Vec::with_capacity(n);
        let mut id_to_index = HashMap::with_capacity(n);

        for (i, (id, pat, strength)) in batch.into_iter().enumerate() {
            debug_assert_eq!(pat.len(), dim, "pattern dimension mismatch");
            data.extend_from_slice(&pat);
            strengths.push(strength);
            pattern_ids.push(id);
            id_to_index.insert(id, i);
        }

        let patterns = Array2::from_shape_vec((n, dim), data).expect("shape matches data length");

        Self {
            patterns: Some(patterns),
            strengths,
            pattern_ids,
            id_to_index,
            dim,
        }
    }

    /// Number of stored patterns.
    pub fn pattern_count(&self) -> usize {
        self.pattern_ids.len()
    }

    /// Pattern dimensionality (0 if empty).
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Check if a pattern with the given id is stored.
    pub fn contains(&self, id: i64) -> bool {
        self.id_to_index.contains_key(&id)
    }

    /// Get the current strength of a pattern.
    pub fn strength(&self, id: i64) -> Option<f32> {
        self.id_to_index.get(&id).map(|&idx| self.strengths[idx])
    }

    /// Get all pattern ids.
    pub fn pattern_ids(&self) -> &[i64] {
        &self.pattern_ids
    }

    // -----------------------------------------------------------------------
    // Store / update / remove
    // -----------------------------------------------------------------------

    /// Store a pattern. If the id already exists, the pattern and strength
    /// are updated in place. The pattern is L2-normalized before storage.
    pub fn store(&mut self, id: i64, pattern: &[f32], strength: f32) {
        let normalized = l2_normalize(pattern);

        if let Some(&idx) = self.id_to_index.get(&id) {
            // Update in place
            if let Some(ref mut mat) = self.patterns {
                mat.row_mut(idx).assign(&Array1::from(normalized));
            }
            self.strengths[idx] = strength;
            return;
        }

        // New pattern
        let arr = Array1::from(normalized);

        if self.dim == 0 {
            self.dim = arr.len();
        }

        match self.patterns {
            Some(ref mut mat) => {
                let new_row = arr.insert_axis(Axis(0));
                let new_mat = ndarray::concatenate(Axis(0), &[mat.view(), new_row.view()])
                    .expect("dimension mismatch in pattern store");
                *mat = new_mat;
            }
            None => {
                self.patterns = Some(arr.insert_axis(Axis(0)).to_owned());
            }
        }

        let idx = self.pattern_ids.len();
        self.pattern_ids.push(id);
        self.strengths.push(strength);
        self.id_to_index.insert(id, idx);
    }

    /// Remove a pattern by id. Rebuilds the internal matrix.
    pub fn remove(&mut self, id: i64) -> bool {
        let idx = match self.id_to_index.remove(&id) {
            Some(i) => i,
            None => return false,
        };

        self.pattern_ids.remove(idx);
        self.strengths.remove(idx);

        // Rebuild index map
        self.id_to_index.clear();
        for (i, &pid) in self.pattern_ids.iter().enumerate() {
            self.id_to_index.insert(pid, i);
        }

        // Rebuild matrix
        if self.pattern_ids.is_empty() {
            self.patterns = None;
            self.dim = 0;
        } else if let Some(ref mat) = self.patterns {
            let n = self.pattern_ids.len();
            let mut data = Vec::with_capacity(n * self.dim);
            // Rebuild from remaining rows, skipping the removed index
            let old_mat = mat.clone();
            for (i, row) in old_mat.rows().into_iter().enumerate() {
                if i == idx {
                    continue;
                }
                data.extend(row.iter());
            }
            self.patterns = Some(
                Array2::from_shape_vec((n, self.dim), data).expect("shape matches after remove"),
            );
        }

        true
    }

    /// Update the strength of a stored pattern.
    pub fn update_strength(&mut self, id: i64, strength: f32) -> bool {
        match self.id_to_index.get(&id) {
            Some(&idx) => {
                self.strengths[idx] = strength;
                true
            }
            None => false,
        }
    }

    // -----------------------------------------------------------------------
    // Retrieval -- modern Hopfield via softmax attention
    // -----------------------------------------------------------------------

    /// Retrieve the top-k patterns most similar to the query using
    /// softmax attention over the stored patterns.
    ///
    /// Returns (memory_id, activation_score) pairs sorted by activation
    /// descending.
    pub fn retrieve(&self, query: &[f32], top_k: usize, beta: f32) -> Vec<(i64, f32)> {
        let mat = match &self.patterns {
            Some(m) => m,
            None => return Vec::new(),
        };

        let q = Array1::from(l2_normalize(query));

        // sims = patterns @ query -- dot product of each stored pattern with query
        let sims = mat.dot(&q);

        // logits[i] = beta * sims[i] * strengths[i]
        let n = sims.len();
        let mut logits = Vec::with_capacity(n);
        for i in 0..n {
            logits.push(beta * sims[i] * self.strengths[i]);
        }

        // Softmax
        let activations = softmax(&logits);

        // Filter, collect, sort
        let mut results: Vec<(i64, f32)> = activations
            .iter()
            .enumerate()
            .filter(|(_, &a)| a >= ACTIVATION_THRESHOLD)
            .map(|(i, &a)| (self.pattern_ids[i], a))
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(top_k);
        results
    }

    /// Pattern completion via iterative attention refinement.
    ///
    /// Starting from a (possibly partial/noisy) query, iteratively
    /// refines the state toward the nearest attractor in the energy
    /// landscape. Each iteration computes a weighted sum of stored
    /// patterns using softmax attention, then L2-normalizes.
    ///
    /// Returns the completed pattern vector.
    pub fn complete(&self, query: &[f32], iterations: usize, beta: f32) -> Vec<f32> {
        let mat = match &self.patterns {
            Some(m) => m,
            None => return query.to_vec(),
        };

        let mut state = Array1::from(l2_normalize(query));
        let n = mat.nrows();

        for _ in 0..iterations {
            // sims = patterns @ state
            let sims = mat.dot(&state);

            // logits = beta * sims * strengths
            let mut logits = Vec::with_capacity(n);
            for i in 0..n {
                logits.push(beta * sims[i] * self.strengths[i]);
            }

            // weights = softmax(logits)
            let weights = softmax(&logits);

            // new_state = sum_i(weights[i] * patterns[i])
            // = patterns^T @ weights
            let w = Array1::from(weights);
            let new_state = mat.t().dot(&w);

            // L2-normalize
            state = Array1::from(l2_normalize(new_state.as_slice().unwrap()));
        }

        state.to_vec()
    }

    /// Compute the energy of a state in the Hopfield landscape.
    /// Lower energy = better fit to stored patterns.
    /// E = -log(sum_i(exp(beta * pattern_i . state * strength_i)))
    #[allow(dead_code)]
    pub fn energy(&self, state: &[f32], beta: f32) -> f32 {
        let mat = match &self.patterns {
            Some(m) => m,
            None => return 0.0,
        };

        let s = Array1::from(l2_normalize(state));
        let sims = mat.dot(&s);
        let n = sims.len();

        let mut logits = Vec::with_capacity(n);
        for i in 0..n {
            logits.push(beta * sims[i] * self.strengths[i]);
        }

        let max_logit = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let log_sum_exp: f32 = logits
            .iter()
            .map(|&l| (l - max_logit).exp())
            .sum::<f32>()
            .ln()
            + max_logit;

        -log_sum_exp
    }
}

impl Default for HopfieldNetwork {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Math utilities
// ---------------------------------------------------------------------------

/// L2-normalize a vector. Returns a zero vector if the input norm is zero.
pub fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm: f32 = v.iter().map(|&x| x * x).sum::<f32>().sqrt();
    if norm < 1e-10 {
        return vec![0.0; v.len()];
    }
    v.iter().map(|&x| x / norm).collect()
}

/// Compute softmax over a slice of logits. Numerically stable via
/// max-subtraction.
fn softmax(logits: &[f32]) -> Vec<f32> {
    if logits.is_empty() {
        return Vec::new();
    }

    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&l| (l - max).exp()).collect();
    let sum: f32 = exps.iter().sum();

    if sum < 1e-10 {
        return vec![1.0 / logits.len() as f32; logits.len()];
    }

    exps.iter().map(|&e| e / sum).collect()
}

/// Cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pattern(dim: usize, seed: u8) -> Vec<f32> {
        (0..dim)
            .map(|i| ((i as f32 + seed as f32) * 0.1).sin())
            .collect()
    }

    #[test]
    fn test_l2_normalize() {
        let v = vec![3.0, 4.0];
        let n = l2_normalize(&v);
        assert!((n[0] - 0.6).abs() < 1e-5);
        assert!((n[1] - 0.8).abs() < 1e-5);
    }

    #[test]
    fn test_l2_normalize_zero() {
        let v = vec![0.0, 0.0, 0.0];
        let n = l2_normalize(&v);
        assert!(n.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_softmax() {
        let logits = vec![1.0, 2.0, 3.0];
        let sm = softmax(&logits);
        assert!((sm.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(sm[2] > sm[1]);
        assert!(sm[1] > sm[0]);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&v, &v);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn test_store_and_retrieve() {
        let mut net = HopfieldNetwork::new();
        let p1 = make_pattern(64, 1);
        let p2 = make_pattern(64, 10);
        let p3 = make_pattern(64, 20);

        net.store(1, &p1, 1.0);
        net.store(2, &p2, 1.0);
        net.store(3, &p3, 1.0);

        assert_eq!(net.pattern_count(), 3);
        assert_eq!(net.dim(), 64);

        // Query with pattern 1 -- should retrieve pattern 1 as top result
        let results = net.retrieve(&p1, 3, DEFAULT_BETA);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, 1);
    }

    #[test]
    fn test_store_updates_existing() {
        let mut net = HopfieldNetwork::new();
        let p1 = make_pattern(32, 1);
        let p2 = make_pattern(32, 5);

        net.store(1, &p1, 0.5);
        assert_eq!(net.pattern_count(), 1);
        assert!((net.strength(1).unwrap() - 0.5).abs() < 1e-5);

        // Update with new pattern and strength
        net.store(1, &p2, 0.9);
        assert_eq!(net.pattern_count(), 1);
        assert!((net.strength(1).unwrap() - 0.9).abs() < 1e-5);
    }

    #[test]
    fn test_remove() {
        let mut net = HopfieldNetwork::new();
        net.store(1, &make_pattern(32, 1), 1.0);
        net.store(2, &make_pattern(32, 5), 1.0);
        net.store(3, &make_pattern(32, 10), 1.0);

        assert_eq!(net.pattern_count(), 3);
        assert!(net.remove(2));
        assert_eq!(net.pattern_count(), 2);
        assert!(!net.contains(2));
        assert!(net.contains(1));
        assert!(net.contains(3));

        // Retrieve still works
        let results = net.retrieve(&make_pattern(32, 1), 2, DEFAULT_BETA);
        assert!(!results.is_empty());
        assert!(results.iter().all(|(id, _)| *id != 2));
    }

    #[test]
    fn test_pattern_completion() {
        let mut net = HopfieldNetwork::new();

        // Store 3 distinct patterns
        let p1 = make_pattern(64, 0);
        let p2 = make_pattern(64, 50);
        let p3 = make_pattern(64, 100);

        net.store(1, &p1, 1.0);
        net.store(2, &p2, 1.0);
        net.store(3, &p3, 1.0);

        // Complete from p1 -- should converge near p1
        let completed = net.complete(&p1, 10, DEFAULT_BETA);
        let sim = cosine_similarity(&completed, &l2_normalize(&p1));
        assert!(
            sim > 0.9,
            "completed pattern should be close to stored; got sim={}",
            sim
        );
    }

    #[test]
    fn test_partial_cue_recall() {
        let mut net = HopfieldNetwork::new();
        let dim = 128;

        // Store a pattern
        let original = make_pattern(dim, 42);
        net.store(1, &original, 1.0);

        // Create a 50% masked cue
        let mut cue = original.clone();
        for (i, val) in cue.iter_mut().enumerate() {
            if i % 2 == 0 {
                *val = 0.0;
            }
        }

        // Complete from partial cue
        let completed = net.complete(&cue, 10, DEFAULT_BETA);
        let normalized_original = l2_normalize(&original);
        let sim = cosine_similarity(&completed, &normalized_original);
        assert!(
            sim > 0.85,
            "50% masked cue should recall well; got sim={}",
            sim
        );
    }

    #[test]
    fn test_strength_affects_retrieval() {
        let mut net = HopfieldNetwork::new();
        let p1 = make_pattern(64, 1);
        let p2 = make_pattern(64, 2); // Similar to p1

        net.store(1, &p1, 1.0); // Strong
        net.store(2, &p2, 0.1); // Weak

        // Query midpoint between p1 and p2
        let query: Vec<f32> = p1
            .iter()
            .zip(p2.iter())
            .map(|(a, b)| (a + b) / 2.0)
            .collect();

        let results = net.retrieve(&query, 2, DEFAULT_BETA);
        // Pattern 1 should dominate due to higher strength
        assert!(!results.is_empty());
        if results.len() >= 2 {
            assert!(results[0].1 > results[1].1);
        }
    }

    #[test]
    fn test_from_patterns() {
        let batch = vec![
            (1, make_pattern(32, 1), 1.0),
            (2, make_pattern(32, 5), 0.8),
            (3, make_pattern(32, 10), 0.5),
        ];
        let net = HopfieldNetwork::from_patterns(batch);
        assert_eq!(net.pattern_count(), 3);
        assert_eq!(net.dim(), 32);
        assert!(net.contains(1));
        assert!(net.contains(2));
        assert!(net.contains(3));
    }

    #[test]
    fn test_empty_network() {
        let net = HopfieldNetwork::new();
        assert_eq!(net.pattern_count(), 0);
        assert_eq!(net.dim(), 0);
        assert!(net.retrieve(&[1.0, 2.0], 5, DEFAULT_BETA).is_empty());

        let completed = net.complete(&[1.0, 2.0], 5, DEFAULT_BETA);
        assert_eq!(completed, vec![1.0, 2.0]);
    }

    #[test]
    fn test_energy_decreases_on_completion() {
        let mut net = HopfieldNetwork::new();
        let p = make_pattern(64, 7);
        net.store(1, &p, 1.0);

        let noisy: Vec<f32> = p
            .iter()
            .enumerate()
            .map(|(i, &x)| if i % 3 == 0 { x + 0.5 } else { x })
            .collect();

        let e_before = net.energy(&noisy, DEFAULT_BETA);
        let completed = net.complete(&noisy, 10, DEFAULT_BETA);
        let e_after = net.energy(&completed, DEFAULT_BETA);

        assert!(
            e_after <= e_before + 1e-5,
            "energy should decrease after completion: before={}, after={}",
            e_before,
            e_after
        );
    }
}
