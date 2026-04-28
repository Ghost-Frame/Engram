use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub(super) struct InjectBody {
    pub session_id: String,
    pub rule_id: String,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PendingQuery {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
pub(super) struct InjectionRow {
    pub id: i64,
    pub session_id: String,
    pub rule_id: String,
    pub severity: String,
    pub message: String,
    pub created_at: String,
}
