use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct BatchRequest {
    pub ops: Vec<BatchOp>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(super) enum BatchOp {
    Store { body: StoreBody },
    Update { body: UpdateBody },
    Link { body: LinkBody },
}

#[derive(Debug, Deserialize)]
pub(super) struct StoreBody {
    pub content: String,
    pub category: Option<String>,
    pub source: Option<String>,
    pub tags: Option<Vec<String>>,
    pub importance: Option<i32>,
    pub is_static: Option<bool>,
    pub session_id: Option<String>,
    pub space_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateBody {
    pub id: i64,
    pub content: Option<String>,
    pub category: Option<String>,
    pub importance: Option<i32>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LinkBody {
    pub source_id: i64,
    pub target_id: i64,
    pub similarity: Option<f64>,
    #[serde(rename = "type")]
    pub link_type: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct BatchResult {
    pub index: usize,
    pub op: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

