use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct CreateApiKeyBody {
    pub name: Option<String>,
    pub scopes: Option<String>,
    pub rate_limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RecordUsageBody {
    pub event_type: String,
    pub quantity: Option<i64>,
    pub agent_id: Option<i64>,
}
