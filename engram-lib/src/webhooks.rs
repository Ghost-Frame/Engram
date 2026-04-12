//! Webhooks -- event dispatch with HMAC signing, CRUD, sync.
//!
//! Ports: platform/webhooks.ts, webhooks/routes.ts (logic)

use crate::db::Database;
use crate::{EngError, Result};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const WEBHOOK_FAILURE_THRESHOLD: i64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: i64,
    pub user_id: i64,
    pub url: String,
    pub events: Vec<String>,
    /// SECURITY: set to `Some(true)` when a shared secret is configured so the
    /// caller can surface "signing enabled" without exposing the material.
    /// The secret itself is never serialized to API responses.
    #[serde(skip_serializing, default)]
    pub secret: Option<String>,
    #[serde(default, rename = "has_secret")]
    pub has_secret: bool,
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

/// Reject webhook URLs that would let the server attack its own network:
/// non-http(s) schemes, loopback, link-local, or RFC1918 private ranges.
/// Callers should invoke this before persisting a webhook.
pub fn validate_webhook_url(raw: &str) -> Result<()> {
    let parsed = url::Url::parse(raw)
        .map_err(|e| EngError::InvalidInput(format!("invalid webhook URL: {}", e)))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(EngError::InvalidInput(
            "webhook URL must use http or https".into(),
        ));
    }
    let host = parsed
        .host()
        .ok_or_else(|| EngError::InvalidInput("webhook URL is missing host".into()))?;
    match host {
        url::Host::Domain(name) => {
            let lower = name.to_ascii_lowercase();
            if lower == "localhost"
                || lower.ends_with(".localhost")
                || lower == "localhost.localdomain"
            {
                return Err(EngError::InvalidInput(
                    "webhook host resolves to loopback".into(),
                ));
            }
            // Reject AWS/GCP metadata hostnames commonly used for SSRF.
            if lower == "metadata.google.internal"
                || lower == "metadata"
                || lower == "metadata.goog"
            {
                return Err(EngError::InvalidInput(
                    "webhook host is a cloud metadata endpoint".into(),
                ));
            }
        }
        url::Host::Ipv4(ip) => {
            let octets = ip.octets();
            if ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                // 169.254/16 link-local incl. AWS metadata 169.254.169.254
                || octets[0] == 169 && octets[1] == 254
                // 100.64/10 CGNAT
                || octets[0] == 100 && (octets[1] & 0xC0) == 64
                // 0.0.0.0/8
                || octets[0] == 0
            {
                return Err(EngError::InvalidInput(format!(
                    "webhook host {} is in a disallowed IPv4 range",
                    ip
                )));
            }
        }
        url::Host::Ipv6(ip) => {
            if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
                return Err(EngError::InvalidInput(format!(
                    "webhook host {} is in a disallowed IPv6 range",
                    ip
                )));
            }
            // ULA fc00::/7
            let segments = ip.segments();
            if segments[0] & 0xfe00 == 0xfc00 {
                return Err(EngError::InvalidInput(
                    "webhook host is in the IPv6 ULA range".into(),
                ));
            }
            // Link-local fe80::/10
            if segments[0] & 0xffc0 == 0xfe80 {
                return Err(EngError::InvalidInput(
                    "webhook host is IPv6 link-local".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Compute `sha256=<hex>` HMAC signature for outbound delivery. Returns the
/// header-ready string so callers don't re-implement the prefix.
fn sign_body(secret: &str, body: &[u8]) -> String {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC-SHA256 accepts any key length");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(7 + digest.len() * 2);
    hex.push_str("sha256=");
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", byte);
    }
    hex
}

pub async fn create_webhook(
    db: &Database,
    url: &str,
    events: &[String],
    secret: Option<&str>,
    user_id: i64,
) -> Result<(i64, String)> {
    // SECURITY/SSRF: reject loopback, private, link-local, and metadata hosts
    // before we ever persist the webhook.
    validate_webhook_url(url)?;
    let events_json = serde_json::to_string(events).unwrap_or_else(|_| "[\"*\"]".to_string());
    let mut rows = db.conn.query(
        "INSERT INTO webhooks (url, events, secret, user_id) VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
        libsql::params![url.to_string(), events_json, secret.map(|s| s.to_string()), user_id],
    ).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("insert webhook failed".into()))?;
    Ok((
        row.get::<i64>(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        row.get::<String>(1)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?,
    ))
}

pub async fn list_webhooks(db: &Database, user_id: i64) -> Result<Vec<Webhook>> {
    let mut rows = db.conn.query(
        "SELECT id, user_id, url, events, secret, is_active, failure_count, last_triggered_at, created_at FROM webhooks WHERE user_id = ?1",
        libsql::params![user_id],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let events_str: String = row.get(3).unwrap_or_else(|_| "[\"*\"]".to_string());
        let events: Vec<String> =
            serde_json::from_str(&events_str).unwrap_or_else(|_| vec!["*".to_string()]);
        // SECURITY: never emit the raw secret from this function. Only record
        // whether one is configured so the API can show "signing enabled".
        let stored_secret: Option<String> = row.get(4).unwrap_or(None);
        let has_secret = stored_secret.as_deref().is_some_and(|s| !s.is_empty());
        result.push(Webhook {
            id: row
                .get(0)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            user_id: row
                .get(1)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            url: row
                .get(2)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            events,
            secret: None,
            has_secret,
            is_active: row.get::<i64>(5).unwrap_or(1) != 0,
            failure_count: row.get(6).unwrap_or(0),
            last_triggered_at: row.get(7).unwrap_or(None),
            created_at: row
                .get(8)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        });
    }
    Ok(result)
}

/// Internal-only: returns webhooks WITH secrets loaded, for delivery paths that
/// need to sign outgoing payloads. Never expose the returned `secret` field to
/// API callers.
async fn list_webhooks_with_secrets(db: &Database, user_id: i64) -> Result<Vec<Webhook>> {
    let mut rows = db.conn.query(
        "SELECT id, user_id, url, events, secret, is_active, failure_count, last_triggered_at, created_at FROM webhooks WHERE user_id = ?1",
        libsql::params![user_id],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let events_str: String = row.get(3).unwrap_or_else(|_| "[\"*\"]".to_string());
        let events: Vec<String> =
            serde_json::from_str(&events_str).unwrap_or_else(|_| vec!["*".to_string()]);
        let secret: Option<String> = row.get(4).unwrap_or(None);
        let has_secret = secret.as_deref().is_some_and(|s| !s.is_empty());
        result.push(Webhook {
            id: row
                .get(0)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            user_id: row
                .get(1)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            url: row
                .get(2)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            events,
            secret,
            has_secret,
            is_active: row.get::<i64>(5).unwrap_or(1) != 0,
            failure_count: row.get(6).unwrap_or(0),
            last_triggered_at: row.get(7).unwrap_or(None),
            created_at: row
                .get(8)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        });
    }
    Ok(result)
}

pub async fn delete_webhook(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn
        .execute(
            "DELETE FROM webhooks WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    Ok(())
}

/// Emit a webhook event to all matching active webhooks for a user.
pub async fn emit_webhook_event(
    db: &Database,
    event: &str,
    payload: &serde_json::Value,
    user_id: i64,
) {
    let hooks = match list_webhooks_with_secrets(db, user_id).await {
        Ok(h) => h,
        Err(_) => return,
    };
    for hook in hooks {
        if !hook.is_active {
            continue;
        }
        if !hook.events.contains(&"*".to_string()) && !hook.events.contains(&event.to_string()) {
            continue;
        }
        // Re-validate the URL at delivery time: a row could have been migrated
        // in from an older build before validation existed.
        if validate_webhook_url(&hook.url).is_err() {
            tracing::warn!(hook_id = hook.id, "skipping webhook with disallowed URL");
            continue;
        }

        let body = serde_json::json!({
            "event": event,
            "timestamp": Utc::now().to_rfc3339(),
            "data": payload,
        });
        let body_str = serde_json::to_string(&body).unwrap_or_default();

        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        if let Some(secret) = hook.secret.as_deref() {
            if !secret.is_empty() {
                let signature = sign_body(secret, body_str.as_bytes());
                headers.push(("X-Engram-Signature".to_string(), signature));
            }
        }

        let url = hook.url.clone();
        let hook_id = hook.id;
        tokio::spawn(async move {
            let client = reqwest::Client::new();
            let mut req = client
                .post(&url)
                .body(body_str)
                .timeout(std::time::Duration::from_secs(10));
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

pub async fn get_changes_since(
    db: &Database,
    since: &str,
    user_id: i64,
    limit: i64,
) -> Result<Vec<SyncChange>> {
    let mut rows = db.conn.query(
        "SELECT id, content, category, source, importance, tags, confidence, sync_id, is_static, is_forgotten, is_archived, version, created_at, updated_at FROM memories WHERE updated_at > ?1 AND user_id = ?2 ORDER BY updated_at ASC LIMIT ?3",
        libsql::params![since.to_string(), user_id, limit],
    ).await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        result.push(SyncChange {
            id: row
                .get(0)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            content: row
                .get(1)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            category: row
                .get(2)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            source: row.get(3).unwrap_or(None),
            importance: row
                .get(4)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            tags: row.get(5).unwrap_or(None),
            confidence: row.get(6).unwrap_or(None),
            sync_id: row.get(7).unwrap_or(None),
            is_static: row.get::<i64>(8).unwrap_or(0) != 0,
            is_forgotten: row.get::<i64>(9).unwrap_or(0) != 0,
            is_archived: row.get::<i64>(10).unwrap_or(0) != 0,
            version: row.get(11).unwrap_or(1),
            created_at: row
                .get(12)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
            updated_at: row
                .get(13)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?,
        });
    }
    Ok(result)
}
