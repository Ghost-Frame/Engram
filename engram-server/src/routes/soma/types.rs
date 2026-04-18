use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct CreateAgentBody {
    pub name: String,
    #[serde(alias = "agent_type", alias = "category")]
    pub r#type: Option<String>,
    pub description: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateAgentBody {
    pub status: Option<String>,
    #[serde(alias = "agent_type", alias = "category")]
    pub r#type: Option<String>,
    pub description: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListAgentsParams {
    #[serde(alias = "type")]
    pub agent_type: Option<String>,
    pub status: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateGroupBody {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AddMemberBody {
    pub agent_id: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct LogEventBody {
    pub level: String,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListLogsParams {
    pub limit: Option<i64>,
}
