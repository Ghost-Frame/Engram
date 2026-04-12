//! Webhooks -- event dispatch with HMAC signing, CRUD, sync.
//!
//! Ports: platform/webhooks.ts, webhooks/routes.ts (logic)

use crate::db::Database;
use crate::{EngError, Result};
use chrono::Utc;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

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

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ---------------------------------------------------------------------------
// SSRF deny-list helpers
// ---------------------------------------------------------------------------

/// Returns true if the IPv4 address falls in a range that should never be
/// reachable from an outbound webhook or proxy request.
pub fn is_ipv4_denied(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        // 169.254/16 link-local incl. AWS metadata 169.254.169.254
        || (octets[0] == 169 && octets[1] == 254)
        // 100.64/10 CGNAT
        || (octets[0] == 100 && (octets[1] & 0xC0) == 64)
        // 0.0.0.0/8
        || octets[0] == 0
}

/// Returns true if the IPv6 address falls in a range that should never be
/// reachable from an outbound webhook or proxy request.
pub fn is_ipv6_denied(ip: &Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    // IPv6-mapped/compat IPv4 (e.g. ::ffff:127.0.0.1)
    if let Some(v4) = ip.to_ipv4_mapped().or_else(|| ip.to_ipv4()) {
        if is_ipv4_denied(&v4) {
            return true;
        }
    }
    let segments = ip.segments();
    // ULA fc00::/7
    if segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }
    // Link-local fe80::/10
    if segments[0] & 0xffc0 == 0xfe80 {
        return true;
    }
    false
}

/// Returns true if the socket address points to a denied IP range.
pub fn is_addr_denied(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(v4) => is_ipv4_denied(&v4),
        IpAddr::V6(v6) => is_ipv6_denied(&v6),
    }
}

/// Reject webhook URLs that would let the server attack its own network:
/// non-http(s) schemes, loopback, link-local, or RFC1918 private ranges.
/// Callers should invoke this before persisting a webhook.
///
/// NOTE: this is a **synchronous** check on literal hostnames and IPs only.
/// For delivery-time validation that also resolves DNS, use
/// [`resolve_and_validate_url`].
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
            if is_ipv4_denied(&ip) {
                return Err(EngError::InvalidInput(format!(
                    "webhook host {} is in a disallowed IPv4 range",
                    ip
                )));
            }
        }
        url::Host::Ipv6(ip) => {
            if is_ipv6_denied(&ip) {
                return Err(EngError::InvalidInput(format!(
                    "webhook host {} is in a disallowed IPv6 range",
                    ip
                )));
            }
        }
    }
    Ok(())
}

/// SECURITY (SSRF-DNS): resolve the hostname in `raw` via DNS and reject the
/// URL if **any** resulting IP falls in a denied range (loopback, private,
/// link-local, CGNAT, cloud metadata, IPv6 ULA). This closes the gap where
/// `validate_webhook_url` only inspects the literal hostname/IP.
///
/// Callers should invoke this at **delivery/request time**, not just at
/// persist time, because DNS can change between the two.
pub async fn resolve_and_validate_url(raw: &str) -> Result<()> {
    // Fast-path: reject obvious bad schemes, literal IPs, and known names.
    validate_webhook_url(raw)?;

    let parsed = url::Url::parse(raw)
        .map_err(|e| EngError::InvalidInput(format!("invalid URL: {}", e)))?;

    // Only domain names need DNS resolution; literal IPs are already
    // validated by the synchronous check above.
    if let Some(url::Host::Domain(name)) = parsed.host() {
        let port = parsed.port_or_known_default().unwrap_or(443);
        let lookup_host = format!("{}:{}", name, port);

        let addrs: Vec<SocketAddr> = tokio::net::lookup_host(&lookup_host)
            .await
            .map_err(|e| {
                EngError::InvalidInput(format!("DNS resolution failed for {}: {}", name, e))
            })?
            .collect();

        if addrs.is_empty() {
            return Err(EngError::InvalidInput(format!(
                "no DNS records for {}",
                name
            )));
        }

        // Reject if ANY resolved address is in a denied range. An attacker
        // could return both a public and a private IP; we cannot control
        // which one the HTTP client will pick.
        for addr in &addrs {
            if is_addr_denied(addr) {
                return Err(EngError::InvalidInput(format!(
                    "DNS for {} resolved to denied address {}",
                    name,
                    addr.ip()
                )));
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
    let url_s = url.to_string();
    let secret_s = secret.map(|s| s.to_string());

    db.write(move |conn| {
        conn.query_row(
            "INSERT INTO webhooks (url, events, secret, user_id) VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
            rusqlite::params![url_s, events_json, secret_s, user_id],
            |row| {
                let id: i64 = row.get(0)?;
                let created_at: String = row.get(1)?;
                Ok((id, created_at))
            },
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

pub async fn list_webhooks(db: &Database, user_id: i64) -> Result<Vec<Webhook>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, url, events, secret, is_active, failure_count, last_triggered_at, created_at \
                 FROM webhooks WHERE user_id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let events_str: String = row
                .get::<_, String>(3)
                .unwrap_or_else(|_| "[\"*\"]".to_string());
            let events: Vec<String> =
                serde_json::from_str(&events_str).unwrap_or_else(|_| vec!["*".to_string()]);
            // SECURITY: never emit the raw secret from this function. Only record
            // whether one is configured so the API can show "signing enabled".
            let stored_secret: Option<String> = row.get(4).unwrap_or(None);
            let has_secret = stored_secret.as_deref().is_some_and(|s| !s.is_empty());
            result.push(Webhook {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                user_id: row.get(1).map_err(rusqlite_to_eng_error)?,
                url: row.get(2).map_err(rusqlite_to_eng_error)?,
                events,
                secret: None,
                has_secret,
                is_active: row.get::<_, i64>(5).unwrap_or(1) != 0,
                failure_count: row.get(6).unwrap_or(0),
                last_triggered_at: row.get(7).unwrap_or(None),
                created_at: row.get(8).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(result)
    })
    .await
}

/// Internal-only: returns webhooks WITH secrets loaded, for delivery paths that
/// need to sign outgoing payloads. Never expose the returned `secret` field to
/// API callers.
async fn list_webhooks_with_secrets(db: &Database, user_id: i64) -> Result<Vec<Webhook>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, user_id, url, events, secret, is_active, failure_count, last_triggered_at, created_at \
                 FROM webhooks WHERE user_id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let events_str: String = row
                .get::<_, String>(3)
                .unwrap_or_else(|_| "[\"*\"]".to_string());
            let events: Vec<String> =
                serde_json::from_str(&events_str).unwrap_or_else(|_| vec!["*".to_string()]);
            let secret: Option<String> = row.get(4).unwrap_or(None);
            let has_secret = secret.as_deref().is_some_and(|s| !s.is_empty());
            result.push(Webhook {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                user_id: row.get(1).map_err(rusqlite_to_eng_error)?,
                url: row.get(2).map_err(rusqlite_to_eng_error)?,
                events,
                secret,
                has_secret,
                is_active: row.get::<_, i64>(5).unwrap_or(1) != 0,
                failure_count: row.get(6).unwrap_or(0),
                last_triggered_at: row.get(7).unwrap_or(None),
                created_at: row.get(8).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(result)
    })
    .await
}

pub async fn delete_webhook(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM webhooks WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
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
        // SECURITY (SSRF-DNS): re-validate at delivery time with DNS
        // resolution. A row could have been migrated from an older build, or
        // the domain's DNS records could have changed to point at private IPs.
        if resolve_and_validate_url(&hook.url).await.is_err() {
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
            // SECURITY (SEC-H2): disable redirect following to prevent
            // X-Engram-Signature header leakage to attacker-controlled hosts
            // via open redirect chains.
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());
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
    let since_s = since.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, category, source, importance, tags, confidence, sync_id, \
                 is_static, is_forgotten, is_archived, version, created_at, updated_at \
                 FROM memories WHERE updated_at > ?1 AND user_id = ?2 ORDER BY updated_at ASC LIMIT ?3",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![since_s, user_id, limit])
            .map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            result.push(SyncChange {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                content: row.get(1).map_err(rusqlite_to_eng_error)?,
                category: row.get(2).map_err(rusqlite_to_eng_error)?,
                source: row.get(3).unwrap_or(None),
                importance: row.get(4).map_err(rusqlite_to_eng_error)?,
                tags: row.get(5).unwrap_or(None),
                confidence: row.get(6).unwrap_or(None),
                sync_id: row.get(7).unwrap_or(None),
                is_static: row.get::<_, i64>(8).unwrap_or(0) != 0,
                is_forgotten: row.get::<_, i64>(9).unwrap_or(0) != 0,
                is_archived: row.get::<_, i64>(10).unwrap_or(0) != 0,
                version: row.get(11).unwrap_or(1),
                created_at: row.get(12).map_err(rusqlite_to_eng_error)?,
                updated_at: row.get(13).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(result)
    })
    .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    // -- is_ipv4_denied unit tests --

    #[test]
    fn ipv4_loopback_denied() {
        assert!(is_ipv4_denied(&Ipv4Addr::LOCALHOST));
        assert!(is_ipv4_denied(&"127.0.0.2".parse().unwrap()));
    }

    #[test]
    fn ipv4_private_denied() {
        assert!(is_ipv4_denied(&"10.0.0.1".parse().unwrap()));
        assert!(is_ipv4_denied(&"172.16.0.1".parse().unwrap()));
        assert!(is_ipv4_denied(&"192.168.1.1".parse().unwrap()));
    }

    #[test]
    fn ipv4_link_local_denied() {
        assert!(is_ipv4_denied(&"169.254.169.254".parse().unwrap()));
        assert!(is_ipv4_denied(&"169.254.0.1".parse().unwrap()));
    }

    #[test]
    fn ipv4_cgnat_denied() {
        assert!(is_ipv4_denied(&"100.64.0.1".parse().unwrap()));
        assert!(is_ipv4_denied(&"100.127.255.255".parse().unwrap()));
    }

    #[test]
    fn ipv4_public_allowed() {
        assert!(!is_ipv4_denied(&"8.8.8.8".parse().unwrap()));
        assert!(!is_ipv4_denied(&"1.1.1.1".parse().unwrap()));
        assert!(!is_ipv4_denied(&"203.0.113.1".parse().unwrap()));
    }

    // -- is_ipv6_denied unit tests --

    #[test]
    fn ipv6_loopback_denied() {
        assert!(is_ipv6_denied(&Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn ipv6_mapped_loopback_denied() {
        assert!(is_ipv6_denied(&"::ffff:127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn ipv6_ula_denied() {
        assert!(is_ipv6_denied(&"fd00::1".parse().unwrap()));
        assert!(is_ipv6_denied(&"fc00::1".parse().unwrap()));
    }

    #[test]
    fn ipv6_link_local_denied() {
        assert!(is_ipv6_denied(&"fe80::1".parse().unwrap()));
    }

    #[test]
    fn ipv6_global_allowed() {
        assert!(!is_ipv6_denied(&"2001:db8::1".parse().unwrap()));
    }

    // -- validate_webhook_url sync tests --

    #[test]
    fn rejects_ftp_scheme() {
        assert!(validate_webhook_url("ftp://example.com/hook").is_err());
    }

    #[test]
    fn rejects_localhost() {
        assert!(validate_webhook_url("http://localhost/hook").is_err());
        assert!(validate_webhook_url("http://sub.localhost/hook").is_err());
    }

    #[test]
    fn rejects_metadata_hosts() {
        assert!(validate_webhook_url("http://metadata.google.internal/latest").is_err());
        assert!(validate_webhook_url("http://metadata/latest").is_err());
    }

    #[test]
    fn rejects_literal_private_ip() {
        assert!(validate_webhook_url("http://127.0.0.1/hook").is_err());
        assert!(validate_webhook_url("http://10.0.0.1/hook").is_err());
        assert!(validate_webhook_url("http://169.254.169.254/latest").is_err());
    }

    #[test]
    fn accepts_public_url() {
        assert!(validate_webhook_url("https://hooks.example.com/callback").is_ok());
    }

    // -- resolve_and_validate_url DNS tests --

    #[tokio::test]
    async fn dns_resolve_rejects_localhost_domain() {
        let result = resolve_and_validate_url("https://localhost/hook").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn dns_resolve_accepts_public_domain() {
        // Skip gracefully if DNS is unavailable.
        match resolve_and_validate_url("https://example.com/webhook").await {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("{}", e);
                if msg.contains("DNS resolution failed") {
                    return;
                }
                panic!("unexpected error: {}", e);
            }
        }
    }
}
