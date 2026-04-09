use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardRule {
    pub id: String,
    pub name: String,
    pub pattern: String,
    pub action: GuardAction,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GuardAction {
    Block,
    Flag,
    Redact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardResult {
    pub allowed: bool,
    pub triggered_rules: Vec<String>,
    pub redacted_content: Option<String>,
}

pub async fn evaluate(_db: &Database, _content: &str) -> Result<GuardResult> {
    todo!()
}

pub async fn create_rule(_db: &Database, _rule: GuardRule) -> Result<GuardRule> {
    todo!()
}

pub async fn list_rules(_db: &Database) -> Result<Vec<GuardRule>> {
    todo!()
}
