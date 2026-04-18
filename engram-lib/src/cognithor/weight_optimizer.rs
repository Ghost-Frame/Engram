use super::types::{WeightState, WeightUpdate};
use std::collections::HashMap;

const ALPHA: f64 = 0.1;
const MIN_WEIGHT: f64 = 0.05;

/// Tracks and auto-tunes search weights per mode based on feedback signals.
#[derive(Debug, Clone)]
pub struct SearchWeightOptimizer {
    pub weights: HashMap<String, WeightState>,
    pub history: Vec<WeightUpdate>,
}

impl Default for SearchWeightOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchWeightOptimizer {
    pub fn new() -> Self {
        Self {
            weights: HashMap::new(),
            history: Vec::new(),
        }
    }

    /// Get current weights for a mode, initializing from defaults if needed.
    pub fn get_weights(&self, mode: &str) -> WeightState {
        self.weights.get(mode).cloned().unwrap_or_else(|| {
            let (vw, fw) = default_weights(mode);
            WeightState {
                vector_weight: vw,
                fts_weight: fw,
                update_count: 0,
            }
        })
    }

    /// Update weights based on a feedback signal.
    /// signal > 0 means vector results were good (boost vector weight).
    /// signal < 0 means FTS results were better (boost FTS weight).
    pub fn update(&mut self, mode: &str, signal: f64) {
        let state = self.weights.entry(mode.to_string()).or_insert_with(|| {
            let (vw, fw) = default_weights(mode);
            WeightState {
                vector_weight: vw,
                fts_weight: fw,
                update_count: 0,
            }
        });

        let old_vector = state.vector_weight;

        // EMA update: nudge vector_weight toward 1.0 or 0.0 based on signal
        let adjustment = ALPHA * signal;
        state.vector_weight =
            (state.vector_weight + adjustment).clamp(MIN_WEIGHT, 1.0 - MIN_WEIGHT);
        state.fts_weight = 1.0 - state.vector_weight;
        state.update_count += 1;

        self.history.push(WeightUpdate {
            mode: mode.to_string(),
            old_vector,
            new_vector: state.vector_weight,
            signal,
            timestamp: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        });

        // Keep history bounded
        if self.history.len() > 1000 {
            self.history.drain(0..500);
        }
    }

    /// Rollback to previous weights for a mode (uses history).
    pub fn rollback(&mut self, mode: &str) -> bool {
        if let Some(last) = self.history.iter().rev().find(|h| h.mode == mode) {
            if let Some(state) = self.weights.get_mut(mode) {
                state.vector_weight = last.old_vector;
                state.fts_weight = 1.0 - last.old_vector;
                return true;
            }
        }
        false
    }
}

fn default_weights(mode: &str) -> (f64, f64) {
    match mode {
        "fact_recall" => (0.62, 0.38),
        "preference" => (0.52, 0.48),
        "reasoning" => (0.50, 0.50),
        "generalization" => (0.48, 0.52),
        "temporal" => (0.35, 0.65),
        _ => (0.50, 0.50),
    }
}
