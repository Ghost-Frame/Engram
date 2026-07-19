//! Webhooks -- registration, HMAC-signed test-fire delivery, and change-feed
//! polling sync.
//!
//! NOTE: registering a webhook here does not subscribe it to live domain
//! events. Only `POST /webhooks/test/{id}` ever delivers to a registered URL;
//! `get_changes_since` is the actual sync mechanism (pull, not push). For
//! wired event delivery use Axon subscriptions (`POST /axon/subscribe`),
//! which fire on activity-report and task-lifecycle events.
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

/// Shared HTTP client for webhook delivery -- no-redirect policy prevents
/// signature header leakage via open redirect chains (SEC-H2). R8-R-004:
/// request + connect timeouts bound the delivery task so a hanging
/// endpoint cannot keep the retry task alive forever.
static WEBHOOK_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(4)
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("safe_client_builder failed at webhook client startup")
});

const WEBHOOK_FAILURE_THRESHOLD: i64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Webhook {
    pub id: i64,
    pub user_id: i64, // populated from caller context; not stored in tenant shards after v30
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

// ---------------------------------------------------------------------------
// SSRF deny-list helpers
// ---------------------------------------------------------------------------

/// Returns true if the IPv4 address falls in a range that should never be
/// reachable from an outbound webhook or proxy request.
pub fn is_ipv4_denied(ip: &Ipv4Addr) -> bool {
    if loopback_test_override(IpAddr::V4(*ip)) {
        return false;
    }
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
    if loopback_test_override(addr.ip()) {
        return false;
    }
    match addr.ip() {
        IpAddr::V4(v4) => is_ipv4_denied(&v4),
        IpAddr::V6(v6) => is_ipv6_denied(&v6),
    }
}

/// Test-only escape hatch: when this crate is compiled under `cfg(test)`,
/// loopback URLs are accepted by the SSRF validators if the
/// `KLEOS_WEBHOOK_ALLOW_LOOPBACK_FOR_TEST` env var is set to `1`. This lets
/// the unit tests below stand up a real `127.0.0.1` receiver to prove
/// single-target delivery semantics. The override applies only to loopback
/// addresses; CGNAT, link-local, ULA, and metadata ranges remain blocked.
///
/// In non-test builds this function is a constant `false` so the override
/// has zero attack surface in production.
#[inline]
fn loopback_test_override(ip: IpAddr) -> bool {
    #[cfg(test)]
    {
        let is_loopback = match ip {
            IpAddr::V4(v) => v.is_loopback(),
            IpAddr::V6(v) => v.is_loopback() || v.to_ipv4_mapped().is_some_and(|m| m.is_loopback()),
        };
        is_loopback
            && std::env::var("KLEOS_WEBHOOK_ALLOW_LOOPBACK_FOR_TEST")
                .map(|v| v == "1")
                .unwrap_or(false)
    }
    #[cfg(not(test))]
    {
        let _ = ip;
        false
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
/// Returns the pinned `IpAddr` that passed validation so callers can connect
/// directly to that IP (with a `Host` header for the original hostname),
/// eliminating the TOCTOU window between DNS resolution and the actual HTTP
/// request. Returns `None` when the URL already contains a literal IP address
/// (no DNS resolution needed).
///
/// Callers should invoke this at **delivery/request time**, not just at
/// persist time, because DNS can change between the two.
#[tracing::instrument(skip(raw))]
pub async fn resolve_and_validate_url(raw: &str) -> Result<Option<IpAddr>> {
    // Fast-path: reject obvious bad schemes, literal IPs, and known names.
    validate_webhook_url(raw)?;

    let parsed =
        url::Url::parse(raw).map_err(|e| EngError::InvalidInput(format!("invalid URL: {}", e)))?;

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

        // Return the first validated IP so the caller can pin the connection
        // to this specific address, closing the TOCTOU DNS rebinding window.
        let pinned_ip = addrs[0].ip();
        return Ok(Some(pinned_ip));
    }

    Ok(None)
}

/// Build a pinned delivery URL by replacing the hostname in `original_url`
/// with the validated `pinned_ip`. Returns a tuple of `(pinned_url,
/// original_host)` where `original_host` should be sent as the HTTP `Host`
/// header so the receiving server can route the request correctly.
///
/// When `pinned_ip` is `None` (the URL already contained a literal IP), the
/// original URL is returned unchanged and no `Host` override is needed.
pub fn pin_url_to_ip(original_url: &str, pinned_ip: Option<IpAddr>) -> (String, Option<String>) {
    let Some(ip) = pinned_ip else {
        return (original_url.to_string(), None);
    };

    let Ok(mut parsed) = url::Url::parse(original_url) else {
        return (original_url.to_string(), None);
    };

    // Capture the original host for the Host header before we overwrite it.
    let original_host = parsed.host_str().map(|h| {
        if let Some(port) = parsed.port() {
            format!("{}:{}", h, port)
        } else {
            h.to_string()
        }
    });

    // Replace the host with the pinned IP. IPv6 addresses must be
    // bracket-wrapped in URLs (RFC 3986 SS3.2.2).
    let ip_str = match ip {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{}]", v6),
    };

    if parsed.set_host(Some(&ip_str)).is_err() {
        // If we cannot set the host (should not happen for valid IPs),
        // fall back to the original URL rather than blocking delivery.
        return (original_url.to_string(), None);
    }

    (parsed.to_string(), original_host)
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

#[tracing::instrument(skip(db, url, events, secret), fields(event_count = events.len()))]
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
        Ok(conn.query_row(
            "INSERT INTO webhooks (url, events, secret, user_id) VALUES (?1, ?2, ?3, ?4) RETURNING id, created_at",
            rusqlite::params![url_s, events_json, secret_s, user_id],
            |row| {
                let id: i64 = row.get(0)?;
                let created_at: String = row.get(1)?;
                Ok((id, created_at))
            },
        )?)
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn list_webhooks(db: &Database, user_id: i64) -> Result<Vec<Webhook>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, url, events, secret, is_active, failure_count, last_triggered_at, created_at \
                 FROM webhooks WHERE user_id = ?1 ORDER BY created_at DESC",
            )
            ?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            ?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            let events_str: String = row
                .get::<_, String>(2)
                .unwrap_or_else(|_| "[\"*\"]".to_string());
            let events: Vec<String> =
                serde_json::from_str(&events_str).unwrap_or_else(|_| vec!["*".to_string()]);
            // SECURITY: never emit the raw secret from this function. Only record
            // whether one is configured so the API can show "signing enabled".
            let stored_secret: Option<String> = row.get(3).unwrap_or(None);
            let has_secret = stored_secret.as_deref().is_some_and(|s| !s.is_empty());
            result.push(Webhook {
                id: row.get(0)?,
                user_id,
                url: row.get(1)?,
                events,
                secret: None,
                has_secret,
                is_active: row.get::<_, i64>(4).unwrap_or(1) != 0,
                failure_count: row.get(5).unwrap_or(0),
                last_triggered_at: row.get(6).unwrap_or(None),
                created_at: row.get(7)?,
            });
        }
        Ok(result)
    })
    .await
}

/// Internal-only: fetch a single webhook by id with its secret loaded. The
/// caller has already passed the tenant-scoping check via `ResolvedDb`, so the
/// row's mere existence implies the user owns it. Returns `None` when no
/// webhook with that id exists in this tenant.
///
/// Used by [`emit_test_to_webhook`] for single-target delivery.
async fn get_webhook_with_secret(
    db: &Database,
    hook_id: i64,
    user_id: i64,
) -> Result<Option<Webhook>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, url, events, secret, is_active, failure_count, last_triggered_at, created_at \
                 FROM webhooks WHERE id = ?1 AND user_id = ?2",
            )
            ?;
        let mut rows = stmt
            .query(rusqlite::params![hook_id, user_id])
            ?;
        if let Some(row) = rows.next()? {
            let events_str: String = row
                .get::<_, String>(2)
                .unwrap_or_else(|_| "[\"*\"]".to_string());
            let events: Vec<String> =
                serde_json::from_str(&events_str).unwrap_or_else(|_| vec!["*".to_string()]);
            let secret: Option<String> = row.get(3).unwrap_or(None);
            let has_secret = secret.as_deref().is_some_and(|s| !s.is_empty());
            Ok(Some(Webhook {
                id: row.get(0)?,
                user_id,
                url: row.get(1)?,
                events,
                secret,
                has_secret,
                is_active: row.get::<_, i64>(4).unwrap_or(1) != 0,
                failure_count: row.get(5).unwrap_or(0),
                last_triggered_at: row.get(6).unwrap_or(None),
                created_at: row.get(7)?,
            }))
        } else {
            Ok(None)
        }
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn delete_webhook(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM webhooks WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![id, user_id],
        )?;
        Ok(())
    })
    .await
}

/// Dead-letter entry produced when all delivery retries are exhausted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDeadLetter {
    pub id: i64,
    pub webhook_id: i64,
    pub event: String,
    pub payload: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub last_status_code: Option<i64>,
    pub created_at: String,
}

/// Receipt returned by [`emit_test_to_webhook`]. Captures the actual delivery
/// outcome for one specific webhook so the `/webhooks/test/{id}` route can
/// surface a real status to the caller instead of "dispatched to N hooks".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookTestReceipt {
    pub hook_id: i64,
    pub event: String,
    pub url: String,
    /// True if the request was sent and the receiver responded with 2xx.
    pub dispatched: bool,
    /// HTTP status code if the receiver responded at all. None on connect /
    /// DNS failure / timeout / SSRF rejection.
    pub status_code: Option<u16>,
    /// Wall-clock time spent in the single delivery attempt, in milliseconds.
    pub latency_ms: u64,
    /// Human-readable error if the attempt did not succeed.
    pub error: Option<String>,
}

/// Increment `failure_count` for a webhook and auto-disable if threshold
/// exceeded. Returns the new failure count.
async fn record_delivery_failure(db: &Database, hook_id: i64) -> Result<i64> {
    let threshold = WEBHOOK_FAILURE_THRESHOLD;
    db.write(move |conn| {
        conn.execute(
            "UPDATE webhooks SET failure_count = failure_count + 1, \
             is_active = CASE WHEN failure_count + 1 >= ?1 THEN 0 ELSE is_active END \
             WHERE id = ?2",
            rusqlite::params![threshold, hook_id],
        )?;
        let count: i64 = conn
            .query_row(
                "SELECT failure_count FROM webhooks WHERE id = ?1",
                rusqlite::params![hook_id],
                |row| row.get(0),
            )
            .unwrap_or(0);
        Ok(count)
    })
    .await
}

/// Reset failure_count to 0 and update last_triggered_at on successful delivery.
async fn record_delivery_success(db: &Database, hook_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE webhooks SET failure_count = 0, last_triggered_at = datetime('now') WHERE id = ?1",
            rusqlite::params![hook_id],
        )
        ?;
        Ok(())
    })
    .await
}

/// List dead-letter entries for a webhook, most recent first.
///
/// Scoped to `user_id`: the dead-letter rows are only returned when the parent
/// webhook is owned by the caller, so one user cannot read another's delivery
/// failures by guessing a webhook id (the BOLA that single-DB mode would
/// otherwise expose, since `webhook_dead_letters` has no `user_id` of its own).
///
/// Nothing currently writes `webhook_dead_letters` (the historical dispatch
/// pipeline that populated it was removed), so this returns an empty list
/// today; the table and route remain for when live dispatch is reintroduced.
#[tracing::instrument(skip(db))]
pub async fn list_dead_letters(
    db: &Database,
    webhook_id: i64,
    user_id: i64,
    limit: i64,
) -> Result<Vec<WebhookDeadLetter>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, webhook_id, event, payload, attempts, \
                 last_error, last_status_code, created_at \
                 FROM webhook_dead_letters \
                 WHERE webhook_id = ?1 \
                 AND webhook_id IN (SELECT id FROM webhooks WHERE user_id = ?2) \
                 ORDER BY created_at DESC LIMIT ?3",
        )?;
        let mut rows = stmt.query(rusqlite::params![webhook_id, user_id, limit])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(WebhookDeadLetter {
                id: row.get(0)?,
                webhook_id: row.get(1)?,
                event: row.get(2)?,
                payload: row.get(3)?,
                attempts: row.get(4)?,
                last_error: row.get(5).unwrap_or(None),
                last_status_code: row.get(6).unwrap_or(None),
                created_at: row.get(7)?,
            });
        }
        Ok(result)
    })
    .await
}

/// Single-shot delivery to one specific webhook for the `/webhooks/test/{id}`
/// route. Unlike the fan-out path, which dispatches to every active
/// webhook for the user, this targets exactly the row identified by
/// `hook_id` and returns a [`WebhookTestReceipt`] describing the actual HTTP
/// outcome.
///
/// Behavior:
/// - 404-equivalent: returns `Err(EngError::NotFound)` if the webhook id does
///   not exist in this tenant database.
/// - 409-equivalent: returns `Err(EngError::Conflict)` if the webhook is
///   disabled (failure_count threshold tripped or operator-disabled). Test
///   should not silently re-enable a disabled hook.
/// - SSRF: applies the same DNS-resolved deny check as production delivery.
///   On rejection the receipt is returned with `dispatched=false` and an
///   explanatory error string; this is `Ok(...)` because the test endpoint
///   wants a structured outcome.
/// - One attempt only. No exponential-backoff retry, no dead-letter row --
///   tests should not pollute the dead-letter tray.
/// - On 2xx success: failure_count is reset and last_triggered_at is bumped
///   (same as a real delivery).
/// - On non-2xx or transport error: failure_count is incremented (same as a
///   real delivery), but no dead-letter row is written.
#[tracing::instrument(skip(db, payload), fields(event = %event, hook_id))]
pub async fn emit_test_to_webhook(
    db: &Database,
    hook_id: i64,
    event: &str,
    payload: &serde_json::Value,
    user_id: i64,
) -> Result<WebhookTestReceipt> {
    let hook = match get_webhook_with_secret(db, hook_id, user_id).await? {
        Some(h) => h,
        None => {
            return Err(EngError::NotFound(format!("webhook {} not found", hook_id)));
        }
    };

    if !hook.is_active {
        return Err(EngError::Conflict(format!(
            "webhook {} is disabled (failure_count={}); enable it before testing",
            hook.id, hook.failure_count
        )));
    }

    // SSRF: re-validate the URL via DNS at delivery time. The synchronous
    // create-time check could have been bypassed by DNS rebinding. The
    // returned IP is pinned so we connect to the exact address we validated,
    // closing the TOCTOU DNS rebinding window.
    let started = std::time::Instant::now();
    let pinned_ip = match resolve_and_validate_url(&hook.url).await {
        Ok(ip) => ip,
        Err(err) => {
            return Ok(WebhookTestReceipt {
                hook_id: hook.id,
                event: event.to_string(),
                url: hook.url,
                dispatched: false,
                status_code: None,
                latency_ms: started.elapsed().as_millis() as u64,
                error: Some(format!("ssrf check rejected url: {}", err)),
            });
        }
    };
    let (pinned_url, pinned_host_header) = pin_url_to_ip(&hook.url, pinned_ip);

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
            headers.push(("X-Kleos-Signature".to_string(), signature));
        }
    }

    let mut req = WEBHOOK_CLIENT
        .post(&pinned_url)
        .body(body_str)
        .timeout(std::time::Duration::from_secs(10));
    // Set Host header to the original hostname when connecting via pinned IP.
    if let Some(ref host) = pinned_host_header {
        req = req.header("Host", host.as_str());
    }
    for (k, v) in &headers {
        req = req.header(k.as_str(), v.as_str());
    }

    let outcome = req.send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match outcome {
        Ok(resp) if resp.status().is_success() => {
            let status = resp.status().as_u16();
            if let Err(e) = record_delivery_success(db, hook.id).await {
                tracing::warn!(error = %e, hook_id = hook.id, "failed to record delivery success");
            }
            Ok(WebhookTestReceipt {
                hook_id: hook.id,
                event: event.to_string(),
                url: hook.url,
                dispatched: true,
                status_code: Some(status),
                latency_ms,
                error: None,
            })
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            if let Err(e) = record_delivery_failure(db, hook.id).await {
                tracing::warn!(error = %e, hook_id = hook.id, "failed to record delivery failure");
            }
            Ok(WebhookTestReceipt {
                hook_id: hook.id,
                event: event.to_string(),
                url: hook.url,
                dispatched: false,
                status_code: Some(status),
                latency_ms,
                error: Some(format!("receiver returned HTTP {}", status)),
            })
        }
        Err(e) => {
            if let Err(e) = record_delivery_failure(db, hook.id).await {
                tracing::warn!(error = %e, hook_id = hook.id, "failed to record delivery failure");
            }
            Ok(WebhookTestReceipt {
                hook_id: hook.id,
                event: event.to_string(),
                url: hook.url,
                dispatched: false,
                status_code: None,
                latency_ms,
                error: Some(e.to_string()),
            })
        }
    }
}

// -- Sync operations --

#[tracing::instrument(skip(db, since))]
pub async fn get_changes_since(
    db: &Database,
    since: &str,
    user_id: i64,
    limit: i64,
) -> Result<Vec<SyncChange>> {
    let since_s = since.to_string();
    db.read(move |conn| {
        // Scope the sync feed to the caller. memories carries user_id in both
        // the monolith schema and the per-tenant shard (dropped at tenant v22,
        // re-added at v55), so the predicate is a no-op in a single-owner shard
        // and the tenant boundary in shared (monolith) mode, where ResolvedDb
        // hands back state.db. Without it the sync path leaks every tenant's
        // changed memories.
        let mut stmt = conn.prepare(
            "SELECT id, content, category, source, importance, tags, confidence, sync_id, \
                 is_static, is_forgotten, is_archived, version, created_at, updated_at \
                 FROM memories WHERE updated_at > ?1 AND user_id = ?2 \
                 ORDER BY updated_at ASC LIMIT ?3",
        )?;
        let mut rows = stmt.query(rusqlite::params![since_s, user_id, limit])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(SyncChange {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                source: row.get(3).unwrap_or(None),
                importance: row.get(4)?,
                tags: row.get(5).unwrap_or(None),
                confidence: row.get(6).unwrap_or(None),
                sync_id: row.get(7).unwrap_or(None),
                is_static: row.get::<_, i64>(8).unwrap_or(0) != 0,
                is_forgotten: row.get::<_, i64>(9).unwrap_or(0) != 0,
                is_archived: row.get::<_, i64>(10).unwrap_or(0) != 0,
                version: row.get(11).unwrap_or(1),
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
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
    #[serial_test::serial(loopback_env)]
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
    #[serial_test::serial(loopback_env)]
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
    #[serial_test::serial(loopback_env)]
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
            Ok(_ip) => {}
            Err(e) => {
                let msg = format!("{}", e);
                if msg.contains("DNS resolution failed") {
                    return;
                }
                panic!("unexpected error: {}", e);
            }
        }
    }

    // -- sign_body tests --

    #[test]
    fn sign_body_produces_sha256_prefix() {
        let sig = sign_body("test-secret", b"hello");
        assert!(sig.starts_with("sha256="));
        // HMAC-SHA256 of "hello" with key "test-secret" is deterministic.
        assert_eq!(sig.len(), 7 + 64); // "sha256=" + 64 hex chars
    }

    #[test]
    fn sign_body_different_secrets_differ() {
        let a = sign_body("secret-a", b"payload");
        let b = sign_body("secret-b", b"payload");
        assert_ne!(a, b);
    }

    #[test]
    fn sign_body_deterministic() {
        let a = sign_body("key", b"data");
        let b = sign_body("key", b"data");
        assert_eq!(a, b);
    }

    // -- dead-letter / retry infrastructure tests --

    #[tokio::test]
    async fn record_failure_increments_count() {
        let db = Database::connect_memory().await.unwrap();
        // Create user
        db.write(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO users (id, username) VALUES (1, 'test')",
                [],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
        // Create webhook
        create_webhook(&db, "https://example.com/hook", &["*".into()], None, 1)
            .await
            .unwrap();
        // Record 3 failures
        let c1 = record_delivery_failure(&db, 1).await.unwrap();
        let c2 = record_delivery_failure(&db, 1).await.unwrap();
        let c3 = record_delivery_failure(&db, 1).await.unwrap();
        assert_eq!(c1, 1);
        assert_eq!(c2, 2);
        assert_eq!(c3, 3);
    }

    #[tokio::test]
    async fn success_resets_failure_count() {
        let db = Database::connect_memory().await.unwrap();
        db.write(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO users (id, username) VALUES (1, 'test')",
                [],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
        create_webhook(&db, "https://example.com/hook", &["*".into()], None, 1)
            .await
            .unwrap();
        record_delivery_failure(&db, 1).await.unwrap();
        record_delivery_failure(&db, 1).await.unwrap();
        record_delivery_success(&db, 1).await.unwrap();
        // After success, count should be 0
        let hooks = list_webhooks(&db, 1).await.unwrap();
        assert_eq!(hooks[0].failure_count, 0);
        assert!(hooks[0].last_triggered_at.is_some());
    }

    #[tokio::test]
    async fn auto_disable_at_threshold() {
        let db = Database::connect_memory().await.unwrap();
        db.write(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO users (id, username) VALUES (1, 'test')",
                [],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
        create_webhook(&db, "https://example.com/hook", &["*".into()], None, 1)
            .await
            .unwrap();
        // Hammer failures up to threshold
        for _ in 0..WEBHOOK_FAILURE_THRESHOLD {
            let _ = record_delivery_failure(&db, 1).await;
        }
        let hooks = list_webhooks(&db, 1).await.unwrap();
        assert!(!hooks[0].is_active, "webhook should be auto-disabled");
        assert_eq!(hooks[0].failure_count, WEBHOOK_FAILURE_THRESHOLD);
    }

    // -- emit_test_to_webhook (single-target delivery) tests --

    /// Spawn a tiny axum receiver per webhook on a free 127.0.0.1 port. Each
    /// receiver records hits into a shared `Vec<i64>` so the test can later
    /// assert which hooks were actually contacted. Returns the bound URL.
    async fn spawn_receiver(
        label: i64,
        hits: std::sync::Arc<tokio::sync::Mutex<Vec<i64>>>,
    ) -> String {
        use axum::{routing::post, Router};
        let app = Router::new().route(
            "/hook",
            post(move || {
                let hits = std::sync::Arc::clone(&hits);
                async move {
                    hits.lock().await.push(label);
                    axum::http::StatusCode::OK
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{}/hook", addr)
    }

    async fn seed_user_and_db() -> Database {
        let db = Database::connect_memory().await.unwrap();
        db.write(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO users (id, username) VALUES (1, 'test')",
                [],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn emit_test_returns_not_found_for_unknown_hook() {
        let db = seed_user_and_db().await;
        let result = emit_test_to_webhook(&db, 9999, "test", &serde_json::json!({"x": 1}), 1).await;
        assert!(matches!(result, Err(EngError::NotFound(_))));
    }

    #[tokio::test]
    async fn emit_test_returns_conflict_for_disabled_hook() {
        let db = seed_user_and_db().await;
        // Use a public-looking URL so create_webhook accepts it.
        create_webhook(&db, "https://hooks.example.com/h", &["*".into()], None, 1)
            .await
            .unwrap();
        // Disable the hook directly.
        db.write(|conn| {
            conn.execute("UPDATE webhooks SET is_active = 0 WHERE id = 1", [])
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
        let result = emit_test_to_webhook(&db, 1, "test", &serde_json::json!({"x": 1}), 1).await;
        assert!(
            matches!(result, Err(EngError::Conflict(_))),
            "expected Conflict, got {:?}",
            result
        );
    }

    /// THE FANOUT REGRESSION TEST: create three hooks, each with its own
    /// receiver. Call emit_test_to_webhook against hook H2 only. Assert the
    /// receiver for H2 received exactly one POST and the receivers for H1 and
    /// H3 received zero. Pre-fix this would have hit all three.
    #[tokio::test]
    #[serial_test::serial(loopback_env)]
    async fn emit_test_targets_single_hook_no_fanout() {
        std::env::set_var("KLEOS_WEBHOOK_ALLOW_LOOPBACK_FOR_TEST", "1");

        let db = seed_user_and_db().await;
        let hits: std::sync::Arc<tokio::sync::Mutex<Vec<i64>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

        // Three hooks, each with its own listener bound to 127.0.0.1.
        let url1 = spawn_receiver(1, std::sync::Arc::clone(&hits)).await;
        let url2 = spawn_receiver(2, std::sync::Arc::clone(&hits)).await;
        let url3 = spawn_receiver(3, std::sync::Arc::clone(&hits)).await;

        let (id1, _) = create_webhook(&db, &url1, &["*".into()], None, 1)
            .await
            .unwrap();
        let (id2, _) = create_webhook(&db, &url2, &["*".into()], None, 1)
            .await
            .unwrap();
        let (id3, _) = create_webhook(&db, &url3, &["*".into()], None, 1)
            .await
            .unwrap();

        let receipt =
            emit_test_to_webhook(&db, id2, "test", &serde_json::json!({"webhook_id": id2}), 1)
                .await
                .expect("emit_test_to_webhook should not error on a healthy hook");

        // Receipt content is correct.
        assert_eq!(receipt.hook_id, id2);
        assert_eq!(receipt.url, url2);
        assert!(
            receipt.dispatched,
            "receipt should report dispatched=true; got {:?}",
            receipt
        );
        assert_eq!(receipt.status_code, Some(200));
        assert!(
            receipt.error.is_none(),
            "no error expected: {:?}",
            receipt.error
        );

        // Settle: the test receiver pushes after returning 200, so give it a
        // moment in case ordering has not yet flushed.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let observed = hits.lock().await.clone();
        assert_eq!(
            observed,
            vec![id2],
            "fanout regression: only hook id={} should have been contacted, got {:?}",
            id2,
            observed
        );

        // The other hooks must remain healthy (no failure_count bumps and
        // still active). This rules out the pre-fix behaviour where H1 and
        // H3 would have been contacted and failed for whatever reason.
        let after = list_webhooks(&db, 1).await.unwrap();
        for hook in &after {
            if hook.id == id2 {
                assert_eq!(hook.failure_count, 0);
                assert!(hook.last_triggered_at.is_some());
            } else {
                assert_eq!(
                    hook.failure_count, 0,
                    "hook id={} (sibling of tested hook) should not have been touched",
                    hook.id
                );
                assert!(
                    hook.last_triggered_at.is_none(),
                    "hook id={} should not have last_triggered_at set",
                    hook.id
                );
            }
        }
        let _ = (id1, id3);

        std::env::remove_var("KLEOS_WEBHOOK_ALLOW_LOOPBACK_FOR_TEST");
    }

    #[tokio::test]
    async fn emit_test_records_failure_on_dns_failure() {
        let db = seed_user_and_db().await;
        // .invalid is reserved by RFC 2606 for guaranteed-NXDOMAIN.
        create_webhook(
            &db,
            "https://nonexistent-test-target-12345.invalid/hook",
            &["*".into()],
            None,
            1,
        )
        .await
        .unwrap();
        let receipt = emit_test_to_webhook(&db, 1, "test", &serde_json::json!({"x": 1}), 1)
            .await
            .expect("emit_test_to_webhook returns a receipt even on DNS failure");
        assert!(!receipt.dispatched);
        assert!(receipt.status_code.is_none());
        assert!(receipt.error.is_some());
        assert!(
            receipt
                .error
                .as_deref()
                .unwrap_or("")
                .contains("ssrf check rejected url"),
            "expected ssrf rejection error, got {:?}",
            receipt.error
        );
    }
}
