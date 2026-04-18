use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TacticalEntry {
    pub key: String,
    pub value: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub failure_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub is_code_query: bool,
    pub detected_language: Option<String>,
    pub recommended_mode: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressedEpisode {
    pub summary: String,
    pub source_memory_ids: Vec<i64>,
    pub result_memory_id: i64,
    pub period_start: String,
    pub period_end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightState {
    pub vector_weight: f64,
    pub fts_weight: f64,
    pub update_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightUpdate {
    pub mode: String,
    pub old_vector: f64,
    pub new_vector: f64,
    pub signal: f64,
    pub timestamp: String,
}
