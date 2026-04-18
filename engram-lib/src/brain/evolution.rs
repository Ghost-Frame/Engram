// ============================================================================
// Evolution -- neuro-symbolic weight learning for the brain substrate.
//
// Per-node and per-edge learned weights trained from feedback signals.
// Persisted to brain_meta table as JSON.
// ============================================================================

pub use super::types::FeedbackSignal;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::db::Database;
use crate::{EngError, Result};

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ----------------------------------------------------------------------------
// Core structs
// ----------------------------------------------------------------------------

/// Learned weight state for all nodes and edges observed so far.
/// Serialized to brain_meta for persistence across restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionState {
    pub node_weights: HashMap<i64, f32>,
    pub edge_weights: HashMap<(i64, i64), f32>,
    pub feedback_buffer: Vec<FeedbackSignal>,
    pub learning_rate: f32,
    pub generation: u32,
}

/// Intermediate form for JSON persistence. HashMap keys must be strings in JSON,
/// so we convert i64 -> String and (i64,i64) -> "src,dst".
#[derive(Debug, Serialize, Deserialize)]
struct EvolutionStateSerde {
    node_weights: HashMap<String, f32>,
    edge_weights: HashMap<String, f32>,
    learning_rate: f32,
    generation: u32,
}

impl EvolutionState {
    /// Default state: no learned weights, learning rate 0.01, generation 0.
    pub fn new() -> Self {
        EvolutionState {
            node_weights: HashMap::new(),
            edge_weights: HashMap::new(),
            feedback_buffer: Vec::new(),
            learning_rate: 0.01,
            generation: 0,
        }
    }

    /// Push a feedback signal into the buffer. Call `train_step` to process.
    pub fn record_feedback(&mut self, signal: FeedbackSignal) {
        self.feedback_buffer.push(signal);
    }

    /// Process all buffered feedback signals and increment the generation counter.
    ///
    /// Positive signal: weight += learning_rate, capped at 2.0
    /// Negative signal: weight -= learning_rate, floored at 0.1
    pub fn train_step(&mut self) {
        let signals: Vec<FeedbackSignal> = self.feedback_buffer.drain(..).collect();

        for signal in &signals {
            let delta = if signal.useful {
                self.learning_rate
            } else {
                -self.learning_rate
            };

            for &id in &signal.memory_ids {
                let w = self.node_weights.entry(id).or_insert(1.0);
                *w = (*w + delta).clamp(0.1, 2.0);
            }

            for &pair in &signal.edge_pairs {
                let w = self.edge_weights.entry(pair).or_insert(1.0);
                *w = (*w + delta).clamp(0.1, 2.0);
            }
        }

        self.generation += 1;
    }

    /// Returns the learned weight for a node, or 1.0 if not yet observed.
    pub fn get_node_weight(&self, id: i64) -> f32 {
        *self.node_weights.get(&id).unwrap_or(&1.0)
    }

    /// Returns the learned weight for an edge, or 1.0 if not yet observed.
    pub fn get_edge_weight(&self, source: i64, target: i64) -> f32 {
        *self.edge_weights.get(&(source, target)).unwrap_or(&1.0)
    }

    // ---- JSON bridge helpers ----

    fn to_serde(&self) -> EvolutionStateSerde {
        EvolutionStateSerde {
            node_weights: self
                .node_weights
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            edge_weights: self
                .edge_weights
                .iter()
                .map(|((s, t), v)| (format!("{},{}", s, t), *v))
                .collect(),
            learning_rate: self.learning_rate,
            generation: self.generation,
        }
    }

    fn from_serde(s: EvolutionStateSerde) -> Self {
        let node_weights: HashMap<i64, f32> = s
            .node_weights
            .iter()
            .filter_map(|(k, v)| k.parse::<i64>().ok().map(|id| (id, *v)))
            .collect();

        let edge_weights: HashMap<(i64, i64), f32> = s
            .edge_weights
            .iter()
            .filter_map(|(k, v)| {
                let mut parts = k.splitn(2, ',');
                let a = parts.next()?.parse::<i64>().ok()?;
                let b = parts.next()?.parse::<i64>().ok()?;
                Some(((a, b), *v))
            })
            .collect();

        EvolutionState {
            node_weights,
            edge_weights,
            feedback_buffer: Vec::new(),
            learning_rate: s.learning_rate,
            generation: s.generation,
        }
    }

    // ---- Persistence ----

    /// Serialize and upsert into brain_meta table.
    pub async fn save_state(&self, db: &Database) -> Result<()> {
        let serde_form = self.to_serde();
        let json_str = serde_json::to_string(&serde_form)
            .map_err(|e| EngError::Internal(format!("evolution serialize failed: {}", e)))?;

        db.write(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO brain_meta (key, value) \
                 VALUES ('evolution_state', ?1)",
                rusqlite::params![json_str],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await
    }

    /// Load from brain_meta table. Returns a fresh default state if not found
    /// or if deserialization fails.
    pub async fn load_state(db: &Database) -> Self {
        let result = db
            .read(|conn| {
                use rusqlite::OptionalExtension;
                let row: rusqlite::Result<Option<String>> = conn
                    .query_row(
                        "SELECT value FROM brain_meta WHERE key = 'evolution_state'",
                        [],
                        |row| row.get(0),
                    )
                    .optional();
                row.map_err(rusqlite_to_eng_error)
            })
            .await;

        match result {
            Ok(Some(json_str)) => match serde_json::from_str::<EvolutionStateSerde>(&json_str) {
                Ok(s) => Self::from_serde(s),
                Err(_) => Self::new(),
            },
            _ => Self::new(),
        }
    }
}

impl Default for EvolutionState {
    fn default() -> Self {
        Self::new()
    }
}

// ----------------------------------------------------------------------------
// Stats result (returned by the evolution/stats endpoint)
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionStatsResult {
    pub generation: u32,
    pub num_node_weights: usize,
    pub num_edge_weights: usize,
    pub learning_rate: f32,
}

impl From<&EvolutionState> for EvolutionStatsResult {
    fn from(s: &EvolutionState) -> Self {
        EvolutionStatsResult {
            generation: s.generation,
            num_node_weights: s.node_weights.len(),
            num_edge_weights: s.edge_weights.len(),
            learning_rate: s.learning_rate,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_weights() {
        let state = EvolutionState::new();
        assert_eq!(state.get_node_weight(42), 1.0);
        assert_eq!(state.get_edge_weight(1, 2), 1.0);
        assert_eq!(state.learning_rate, 0.01);
        assert_eq!(state.generation, 0);
    }

    #[test]
    fn test_positive_feedback() {
        let mut state = EvolutionState::new();
        let signal = FeedbackSignal {
            memory_ids: vec![1, 2],
            edge_pairs: vec![(1, 2)],
            useful: true,
            timestamp: 0.0,
        };
        state.record_feedback(signal);
        state.train_step();

        let expected = 1.0_f32 + 0.01;
        assert!(
            (state.get_node_weight(1) - expected).abs() < 1e-6,
            "node 1 weight expected ~{}, got {}",
            expected,
            state.get_node_weight(1)
        );
        assert!(
            (state.get_node_weight(2) - expected).abs() < 1e-6,
            "node 2 weight expected ~{}, got {}",
            expected,
            state.get_node_weight(2)
        );
        assert!(
            (state.get_edge_weight(1, 2) - expected).abs() < 1e-6,
            "edge (1,2) weight expected ~{}, got {}",
            expected,
            state.get_edge_weight(1, 2)
        );
        assert_eq!(state.generation, 1);
    }

    #[test]
    fn test_negative_feedback() {
        let mut state = EvolutionState::new();
        let signal = FeedbackSignal {
            memory_ids: vec![5],
            edge_pairs: vec![(5, 6)],
            useful: false,
            timestamp: 0.0,
        };
        state.record_feedback(signal);
        state.train_step();

        let expected = 1.0_f32 - 0.01;
        assert!(
            (state.get_node_weight(5) - expected).abs() < 1e-6,
            "node 5 weight expected ~{}, got {}",
            expected,
            state.get_node_weight(5)
        );
        assert!(
            (state.get_edge_weight(5, 6) - expected).abs() < 1e-6,
            "edge (5,6) weight expected ~{}, got {}",
            expected,
            state.get_edge_weight(5, 6)
        );
    }

    #[test]
    fn test_weight_bounds() {
        let mut state = EvolutionState::new();
        for _ in 0..200 {
            let signal = FeedbackSignal {
                memory_ids: vec![99],
                edge_pairs: vec![(99, 100)],
                useful: true,
                timestamp: 0.0,
            };
            state.record_feedback(signal);
            state.train_step();
        }
        assert!(
            state.get_node_weight(99) <= 2.0,
            "node weight must not exceed 2.0, got {}",
            state.get_node_weight(99)
        );
        assert!(
            (state.get_node_weight(99) - 2.0).abs() < 1e-4,
            "node weight should be at ceiling 2.0, got {}",
            state.get_node_weight(99)
        );

        let mut state2 = EvolutionState::new();
        for _ in 0..200 {
            let signal = FeedbackSignal {
                memory_ids: vec![77],
                edge_pairs: vec![(77, 78)],
                useful: false,
                timestamp: 0.0,
            };
            state2.record_feedback(signal);
            state2.train_step();
        }
        assert!(
            state2.get_node_weight(77) >= 0.1,
            "node weight must not go below 0.1, got {}",
            state2.get_node_weight(77)
        );
        assert!(
            (state2.get_node_weight(77) - 0.1).abs() < 1e-4,
            "node weight should be at floor 0.1, got {}",
            state2.get_node_weight(77)
        );
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut state = EvolutionState::new();
        state.node_weights.insert(10, 1.5);
        state.node_weights.insert(20, 0.7);
        state.edge_weights.insert((10, 20), 1.3);
        state.generation = 7;

        let serde_form = state.to_serde();
        let loaded = EvolutionState::from_serde(serde_form);

        assert_eq!(loaded.generation, 7);
        assert!(
            (loaded.get_node_weight(10) - 1.5).abs() < 1e-6,
            "node 10 mismatch: got {}",
            loaded.get_node_weight(10)
        );
        assert!(
            (loaded.get_node_weight(20) - 0.7).abs() < 1e-6,
            "node 20 mismatch: got {}",
            loaded.get_node_weight(20)
        );
        assert!(
            (loaded.get_edge_weight(10, 20) - 1.3).abs() < 1e-6,
            "edge (10,20) mismatch: got {}",
            loaded.get_edge_weight(10, 20)
        );
        assert_eq!(loaded.get_node_weight(999), 1.0);
    }
}
