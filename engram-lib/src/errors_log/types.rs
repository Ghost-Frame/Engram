use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEvent {
    pub id: i64,
    pub source: String,
    pub level: String,
    pub message: String,
    pub context: Option<String>,
    pub created_at: String,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LogErrorRequest {
    pub source: String,
    pub level: String,
    pub message: String,
    pub context: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListErrorsRequest {
    pub level: Option<String>,
    pub source: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}
