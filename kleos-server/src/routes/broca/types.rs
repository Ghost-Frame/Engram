use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct LogActionBody {
    pub agent: String,
    pub service: Option<String>,
    pub action: Option<String>,
    pub summary: Option<String>,
    pub detail: Option<String>,
    pub narrative: Option<String>,
    pub project: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub axon_event_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct QueryActionsParams {
    pub agent: Option<String>,
    pub service: Option<String>,
    pub action: Option<String>,
    pub since: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
