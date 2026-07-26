use serde::Deserialize;
use serde_json::Value;

/// Carries pagination parameters for graph listing endpoints.
#[derive(Debug, Deserialize)]
pub(super) struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Accepts mutable fields for updating an entity.
#[derive(Debug, Deserialize)]
pub(super) struct UpdateEntityBody {
    pub name: Option<String>,
    pub entity_type: Option<String>,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

/// Accepts an entity-neighborhood search query.
#[derive(Debug, Deserialize)]
pub(super) struct EntitySearchBody {
    pub query: String,
    pub limit: Option<i64>,
}

/// Filters relationship operations by relationship type.
#[derive(Debug, Deserialize)]
pub(super) struct RelationshipQuery {
    #[serde(rename = "type")]
    pub relationship_type: Option<String>,
}

/// Identifies a relationship edge to delete.
#[derive(Debug, Deserialize)]
pub(super) struct DeleteRelationshipBody {
    pub target_entity_id: i64,
    #[serde(rename = "type")]
    pub relationship_type: Option<String>,
}

/// Carries fact-list query filters.
#[derive(Debug, Deserialize)]
pub(super) struct FactsQuery {
    pub limit: Option<usize>,
    pub memory_id: Option<i64>,
}

/// Carries graph build query parameters for GUI and API callers.
#[derive(Debug, Deserialize)]
pub(super) struct GraphQuery {
    pub limit: Option<i64>,
    pub max: Option<i64>,
    pub min_component: Option<usize>,
    pub depth: Option<i64>,
    #[allow(dead_code)]
    pub offset: Option<i64>,
}

/// Accepts a graph search query.
#[derive(Debug, Deserialize)]
pub(super) struct GraphSearchBody {
    pub query: String,
    pub limit: Option<usize>,
}

/// Carries graph neighborhood traversal options.
#[derive(Debug, Deserialize)]
pub(super) struct NeighborhoodQuery {
    pub depth: Option<u32>,
    /// Comma-separated link types to filter traversal (e.g. "similarity,cite").
    /// If omitted, all link types are traversed.
    pub link_types: Option<String>,
}
