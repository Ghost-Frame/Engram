use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntheticMemory {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub importance: i32,
    pub created_at: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyntheticEdge {
    pub source_id: i64,
    pub target_id: i64,
    pub weight: f32,
    pub edge_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstinctsCorpus {
    pub version: u32,
    pub generated_at: String,
    pub memories: Vec<SyntheticMemory>,
    pub edges: Vec<SyntheticEdge>,
}
