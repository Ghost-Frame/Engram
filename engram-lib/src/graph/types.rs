use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    Cite,
    Mentions,
    Contradicts,
    Refines,
    Generalizes,
    HasFact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLink {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub link_type: LinkType,
    pub weight: f32,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub weight: f32,
    pub pagerank: Option<f32>,
    pub community: Option<u32>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub link_type: LinkType,
    pub weight: f32,
}
