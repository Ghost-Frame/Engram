use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct ListQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateEntityBody {
    pub name: Option<String>,
    pub entity_type: Option<String>,
    pub description: Option<String>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct EntitySearchBody {
    pub query: String,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RelationshipQuery {
    #[serde(rename = "type")]
    pub relationship_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DeleteRelationshipBody {
    pub target_entity_id: i64,
    #[serde(rename = "type")]
    pub relationship_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct FactsQuery {
    pub limit: Option<usize>,
    pub memory_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphQuery {
    pub limit: Option<i64>,
    pub max: Option<i64>,
    #[allow(dead_code)]
    pub depth: Option<i64>,
    #[allow(dead_code)]
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GraphSearchBody {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct NeighborhoodQuery {
    pub depth: Option<u32>,
    /// Comma-separated link types to filter traversal (e.g. "similarity,cite").
    /// If omitted, all link types are traversed.
    pub link_types: Option<String>,
}
