use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct CreateSessionBody {
    pub name: Option<String>,
    pub backend: Option<String>,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ToolsQuery {
    #[allow(dead_code)]
    pub refresh: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(super) struct ExecuteBody {
    pub tool: String,
    pub args: Option<Value>,
    pub session_id: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct QualityQuery {
    pub limit: Option<usize>,
    pub degraded: Option<String>,
}
