use serde::{Deserialize, Serialize};
use std::fmt;

/// Types of connections between brain patterns. Mirrors the eidolon
/// edge taxonomy: association (cosine similarity), temporal (co-occurrence
/// within a time window), contradiction (high sim + same category +
/// different content), causal (NLP-scored cause-effect), and resolves
/// (a memory that resolves/supersedes a contradiction or prior belief).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    Association,
    Temporal,
    Contradiction,
    Causal,
    /// This memory resolves or supersedes the target memory.
    Resolves,
}

impl fmt::Display for EdgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EdgeType::Association => write!(f, "association"),
            EdgeType::Temporal => write!(f, "temporal"),
            EdgeType::Contradiction => write!(f, "contradiction"),
            EdgeType::Causal => write!(f, "causal"),
            EdgeType::Resolves => write!(f, "resolves"),
        }
    }
}

impl EdgeType {
    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "temporal" => EdgeType::Temporal,
            "contradiction" => EdgeType::Contradiction,
            "causal" => EdgeType::Causal,
            "resolves" => EdgeType::Resolves,
            _ => EdgeType::Association,
        }
    }
}

/// A weighted, typed edge between two brain patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainEdge {
    pub id: i64,
    pub source_id: i64,
    pub target_id: i64,
    pub weight: f32,
    pub edge_type: EdgeType,
    pub user_id: i64,
    pub created_at: String,
}

/// A single pattern stored in the Hopfield substrate. Each pattern
/// corresponds to a memory embedding projected into the brain's vector
/// space. `strength` tracks how "alive" the pattern is (0.0 = dead,
/// 1.0 = fully consolidated).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainPattern {
    pub id: i64,
    pub user_id: i64,
    pub pattern: Vec<f32>,
    pub strength: f32,
    pub importance: i32,
    pub access_count: i32,
    pub last_activated_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResult {
    pub pattern_id: i64,
    pub activation: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayStats {
    pub patterns_decayed: usize,
    pub patterns_removed: usize,
    pub edges_decayed: usize,
    pub edges_removed: usize,
}
