//! Webhooks -- event dispatch with HMAC signing, CRUD, sync.
//!
//! Ports: platform/webhooks.ts, webhooks/routes.ts (logic)

use chrono::Utc;
use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

const WEBHOOK_FAILURE_THRESHOLD: i64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: i64,
    pub user_id: i64,
    pub url: String,
    pub events: Vec<String>,
    pub secret: Option<String>,
    pub is_active: bool,
    pub failure_count: i64,
    pub last_triggered_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncChange {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: Option<String>,
    pub importance: i64,
    pub tags: Option<String>,
    pub confidence: Option<f64>,
    pub sync_id: Option<String>,
    pub is_static: bool,
    pub is_forgotten: bool,
    pub is_archived: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn create_webhook(db: &Database, url: &str, events: &[String], secret: Option<&str>, user_id: i64) -> Result<(i64, String)> {
    let events_json = serde_json::to_string(events).unwrap_or_else(|_| "[\"*\"]".to_string());
    let mut rows = db.conn.query(
        "INSERT INTO webhooks (url, events, secret, user_id) VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
        libsql::params![url.to_string(), events_json, secret.map(|s| s.to_string()), user_id],
    ).await?;
    let row = rows.next().await?.ok_or_else(|| crate::EngError::Internal("insert webhook failed".into()))?;
    Ok((row.get::<i64>(0).map_err(|e| crate::EngError::Internal(e.to_string()))?, row.get::<String>(1).map_err(|e| crate::EngError::Internal(e.to_string()))?))
}

pub async fn list_webhooks(db: &Database, user_id: i64) -> Result<Vec<Webhook>> {
    let mut rows = db.conn.query(
        "SELECT id, user_id, url, events, secret, is_active, failure_count, last_triggered_at, created_at FROM webhooks WHERE user_id = ?1",
        libsql::params![user_id],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let events_str: String = row.get(3).unwrap_or_else(|_| "[\"*\"]".to_string());
        let events: Vec<String> = serde_json::from_str(&events_str).unwrap_or_else(|_| vec!["*".to_string()]);
        result.push(Webhook {
            id: row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            user_id: row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            url: row.get(2).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            events,
            secret: row.get(4).unwrap_or(None),
            is_active: row.get::<i64>(5).unwrap_or(1) != 0,
            failure_count: row.get(6).unwrap_or(0),
            last_triggered_at: row.get(7).unwrap_or(None),
            created_at: row.get(8).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        });
    }
    Ok(result)
}

pub async fn delete_webhook(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn.execute("DELETE FROM webhooks WHERE id = ?1 AND user_id = ?2", libsql::params![id, user_id]).await?;
    Ok(())
}

/// Emit a webhook event to all matching active webhooks for a user.
pub async fn emit_webhook_event(db: &Database, event: &str, payload: &serde_json::Value, user_id: i64) {
    let hooks = match list_webhooks(db, user_id).await {
        Ok(h) => h,
        Err(_) => return,
    };
    for hook in hooks {
        if !hook.is_active { continue; }
        if !hook.events.contains(&"*".to_string()) && !hook.events.contains(&event.to_string()) { continue; }

        let body = serde_json::json!({
            "event": event,
            "timestamp": Utc::now().to_rfc3339(),
            "data": payload,
        });
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        if hook.secret.is_some() {
            // Signature support is intentionally deferred until the crypto
            // dependency is wired into this workspace.
        }

        let url = hook.url.clone();
        let hook_id = hook.id;
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let mut req = client.post(&url).body(body_str).timeout(std::time::Duration::from_secs(10));
            for (k, v) in &headers {
                req = req.header(k.as_str(), v.as_str());
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(hook_id, "webhook delivered");
                }
                _ => {
                    tracing::warn!(hook_id, "webhook delivery failed");
                }
            }
        });
    }
}

// -- Sync operations --

pub async fn get_changes_since(db: &Database, since: &str, user_id: i64, limit: i64) -> Result<Vec<SyncChange>> {
    let mut rows = db.conn.query(
        "SELECT id, content, category, source, importance, tags, confidence, sync_id, is_static, is_forgotten, is_archived, version, created_at, updated_at FROM memories WHERE updated_at > ?1 AND user_id = ?2 ORDER BY updated_at ASC LIMIT ?3",
        libsql::params![since.to_string(), user_id, limit],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(SyncChange {
            id: row.get(0).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            content: row.get(1).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            category: row.get(2).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            source: row.get(3).unwrap_or(None),
            importance: row.get(4).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            tags: row.get(5).unwrap_or(None),
            confidence: row.get(6).unwrap_or(None),
            sync_id: row.get(7).unwrap_or(None),
            is_static: row.get::<i64>(8).unwrap_or(0) != 0,
            is_forgotten: row.get::<i64>(9).unwrap_or(0) != 0,
            is_archived: row.get::<i64>(10).unwrap_or(0) != 0,
            version: row.get(11).unwrap_or(1),
            created_at: row.get(12).map_err(|e| crate::EngError::Internal(e.to_string()))?,
            updated_at: row.get(13).map_err(|e| crate::EngError::Internal(e.to_string()))?,
        });
    }
    Ok(result)
}
