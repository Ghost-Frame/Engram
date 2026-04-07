use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub secret: Option<String>,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
}

pub async fn create_webhook(db: &Database, webhook: Webhook) -> Result<Webhook> {
    todo!()
}

pub async fn list_webhooks(db: &Database) -> Result<Vec<Webhook>> {
    todo!()
}

pub async fn delete_webhook(db: &Database, id: &str) -> Result<()> {
    todo!()
}

pub async fn dispatch(db: &Database, event: &str, payload: &serde_json::Value) -> Result<()> {
    todo!()
}
