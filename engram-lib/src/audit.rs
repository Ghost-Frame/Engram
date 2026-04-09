use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub operation: String,
    pub resource_type: String,
    pub resource_id: String,
    pub actor: Option<String>,
    pub before: Option<serde_json::Value>,
    pub after: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

pub async fn log_mutation(
    _db: &Database,
    _operation: &str,
    _resource_type: &str,
    _resource_id: &str,
    _actor: Option<&str>,
    _before: Option<serde_json::Value>,
    _after: Option<serde_json::Value>,
) -> Result<AuditEntry> {
    todo!()
}

pub async fn query_audit_log(
    _db: &Database,
    _resource_type: Option<&str>,
    _resource_id: Option<&str>,
    _limit: usize,
) -> Result<Vec<AuditEntry>> {
    todo!()
}
