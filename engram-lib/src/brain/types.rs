use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Evolution DTOs (evolution.rs keeps EvolutionState + EvolutionStatsResult)
// ---------------------------------------------------------------------------

/// A single feedback event from a caller indicating whether a set of memories
/// and edges were useful or not.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSignal {
    pub memory_ids: Vec<i64>,
    pub edge_pairs: Vec<(i64, i64)>,
    pub useful: bool,
    pub timestamp: f64,
}

// ---------------------------------------------------------------------------
// PCA DTOs (pca.rs keeps PcaTransform + impl)
// ---------------------------------------------------------------------------

/// Stored PCA model metadata.
#[derive(Debug, Clone)]
pub struct PcaModelRow {
    pub id: i64,
    pub source_dim: i64,
    pub target_dim: i64,
    pub fit_at: String,
    pub model_blob: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Reasoning DTOs
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
