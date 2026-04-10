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
    Association,
    Temporal,
    Causal,
    Resolves,
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

// -- Entity types (used by entities.rs) --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: i64,
    pub name: String,
    pub entity_type: String,
    pub description: Option<String>,
    pub aliases: Option<String>,
    pub user_id: i64,
    pub space_id: Option<i64>,
    pub confidence: f64,
    pub occurrence_count: i64,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRelationship {
    pub id: i64,
    pub source_entity_id: i64,
    pub target_entity_id: i64,
    pub relationship_type: String,
    pub strength: f64,
    pub evidence_count: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMemorySearchResult {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: String,
    pub importance: i32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEntityRequest {
    pub name: String,
    pub entity_type: Option<String>,
    pub description: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub user_id: Option<i64>,
    pub space_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRelationshipRequest {
    pub source_entity_id: i64,
    pub target_entity_id: i64,
    pub relationship_type: Option<String>,
    pub strength: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphBuildOptions {
    #[serde(default)]
    pub user_id: i64,
    pub limit: Option<usize>,
}
