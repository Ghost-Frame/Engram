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
    db: &Database,
    operation: &str,
    resource_type: &str,
    resource_id: &str,
    actor: Option<&str>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
) -> Result<AuditEntry> {
    todo!()
}

pub async fn query_audit_log(
    db: &Database,
    resource_type: Option<&str>,
    resource_id: Option<&str>,
    limit: usize,
) -> Result<Vec<AuditEntry>> {
    todo!()
}
