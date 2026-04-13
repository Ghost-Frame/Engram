use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::Url;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::config::Config;
use crate::cred::pattern::{has_secret_patterns, resolve_patterns, SecretPatternKind};
use crate::db::Database;
use crate::services::broca::{log_action, LogActionRequest};
use crate::{EngError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretAccessMode {
    Resolved,
    Raw,
    Scrub,
}

impl SecretAccessMode {
    fn tier(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Raw => "raw",
            Self::Scrub => "scrub",
        }
    }
}

#[derive(Debug, Clone)]
struct CachedSecret {
    value: String,
    expires_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct FetchSecretRequest<'a> {
    pub service: &'a str,
    pub key: &'a str,
    pub mode: SecretAccessMode,
    pub use_cache: bool,
}

#[derive(Debug, Deserialize)]
struct CreddSecretDocument {
    service: String,
    key: String,
    value: Value,
}

#[derive(Debug, Clone)]
pub struct CreddClient {
    http: reqwest::Client,
    base_url: String,
    agent_key: Option<String>,
    agent_key_env: String,
    allow_raw: bool,
    cache_ttl: Duration,
    cache: Arc<Mutex<HashMap<String, CachedSecret>>>,
    /// SECURITY (SEC-CRIT-3): production clients always refuse to proxy
    /// outbound requests to loopback or private ranges. The test
    /// constructor flips this to true so integration tests can hit a mock
    /// server on 127.0.0.1. `from_config` must never set this.
    pub(crate) allow_loopback_proxy: bool,
}

impl CreddClient {
    pub fn from_config(config: &Config) -> Self {
        Self {
            http: build_http_client(),
            base_url: config.eidolon.credd.url.clone(),
            agent_key: None,
            agent_key_env: config.eidolon.credd.agent_key_env.clone(),
            allow_raw: config.eidolon.credd.allow_raw,
            cache_ttl: Duration::from_secs(config.eidolon.credd.cache_ttl_secs.max(1)),
            cache: Arc::new(Mutex::new(HashMap::new())),
            allow_loopback_proxy: false,
        }
    }

    pub fn for_testing(
        base_url: impl Into<String>,
        agent_key: impl Into<String>,
        allow_raw: bool,
        cache_ttl_secs: u64,
    ) -> Self {
        Self {
            http: build_http_client(),
            base_url: base_url.into(),
            agent_key: Some(agent_key.into()),
            agent_key_env: "CREDD_AGENT_KEY".to_string(),
            allow_raw,
            cache_ttl: Duration::from_secs(cache_ttl_secs.max(1)),
            cache: Arc::new(Mutex::new(HashMap::new())),
            // Tests bind mock upstreams on 127.0.0.1; without this flag
            // the SSRF validator would block them.
            allow_loopback_proxy: true,
        }
    }

    pub fn allow_raw(&self) -> bool {
        self.allow_raw
    }

    /// Base URL the client is pointed at. Used by scrub-cache keys so
    /// two CreddClient instances talking to different upstreams never
    /// share a cache entry.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// SECURITY (SEC-MED-1): cache entries are scoped per tenant. Callers
    /// must pass the user_id so rotation invalidation does not cross
    /// tenants (and, more importantly, one tenant cannot read another
    /// tenant's cached secret by coincidence of service/key).
    pub fn invalidate(&self, user_id: i64, service: &str, key: &str) {
        let mut cache = self.cache.lock().expect("credd cache mutex poisoned");
        cache.remove(&cache_key(user_id, service, key));
    }

    /// Drop every cached entry matching `(service, key)` regardless of
    /// tenant. Useful when a rotation signal arrives without a user_id.
    pub fn invalidate_all_tenants(&self, service: &str, key: &str) {
        let suffix = format!(":{service}/{key}");
        let mut cache = self.cache.lock().expect("credd cache mutex poisoned");
        cache.retain(|k, _| !k.ends_with(&suffix));
    }

    pub fn invalidate_all(&self) {
        let mut cache = self.cache.lock().expect("credd cache mutex poisoned");
        cache.clear();
    }

    pub async fn resolve_text(
        &self,
        db: &Database,
        user_id: i64,
        agent: &str,
        text: &str,
    ) -> Result<String> {
        self.resolve_text_with_options(db, user_id, agent, text, false)
            .await
    }

    pub async fn resolve_text_with_options(
        &self,
        db: &Database,
        user_id: i64,
        agent: &str,
        text: &str,
        allow_raw: bool,
    ) -> Result<String> {
        if !has_secret_patterns(text) {
            return Ok(text.to_string());
        }

        resolve_patterns(text, |pattern| async move {
            match pattern.kind {
                SecretPatternKind::Secret => {
                    self.fetch_secret_value(
                        db,
                        user_id,
                        agent,
                        FetchSecretRequest {
                            service: &pattern.service,
                            key: &pattern.key,
                            mode: SecretAccessMode::Resolved,
                            use_cache: true,
                        },
                    )
                    .await
                }
                SecretPatternKind::Raw => {
                    if !allow_raw {
                        return Err(EngError::Auth(
                            "raw secret placeholders are only allowed on admin-gated flows".into(),
                        ));
                    }
                    self.get_raw(db, user_id, agent, &pattern.service, &pattern.key)
                        .await
                }
            }
        })
        .await
    }

    pub async fn list_secret_values(
        &self,
        db: &Database,
        user_id: i64,
        agent: &str,
    ) -> Result<Vec<String>> {
        let url = self.build_url(&["secrets"])?;
        let response = self
            .http
            .get(url)
            .bearer_auth(self.agent_key()?)
            .send()
            .await
            .map_err(|e| EngError::Internal(format!("credd list request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(EngError::Internal(format!(
                "credd list request failed with status {}",
                response.status()
            )));
        }

        let payload: Value = response
            .json()
            .await
            .map_err(|e| EngError::Internal(format!("invalid credd list response: {}", e)))?;
        let refs = parse_secret_refs(&payload)?;

        let mut secrets = Vec::new();
        for (service, key) in refs {
            secrets.push(
                self.fetch_secret_value(
                    db,
                    user_id,
                    agent,
                    FetchSecretRequest {
                        service: &service,
                        key: &key,
                        mode: SecretAccessMode::Scrub,
                        use_cache: true,
                    },
                )
                .await?,
            );
        }

        Ok(secrets)
    }

    #[cfg(feature = "credd-raw")]
    pub async fn get_raw(
        &self,
        db: &Database,
        user_id: i64,
        agent: &str,
        service: &str,
        key: &str,
    ) -> Result<String> {
        if !self.allow_raw {
            return Err(EngError::Auth(
                "credd raw fetch disabled by runtime config".into(),
            ));
        }

        self.fetch_secret_value(
            db,
            user_id,
            agent,
            FetchSecretRequest {
                service,
                key,
                mode: SecretAccessMode::Raw,
                use_cache: false,
            },
        )
        .await
    }

    #[cfg(not(feature = "credd-raw"))]
    pub async fn get_raw(
        &self,
        _db: &Database,
        _user_id: i64,
        _agent: &str,
        _service: &str,
        _key: &str,
    ) -> Result<String> {
        Err(EngError::NotImplemented(
            "credd raw fetch disabled at compile time".into(),
        ))
    }

    pub(crate) async fn fetch_secret_value(
        &self,
        db: &Database,
        user_id: i64,
        agent: &str,
        request: FetchSecretRequest<'_>,
    ) -> Result<String> {
        if request.use_cache {
            // SECURITY (SEC-MED-1): cache lookups must include user_id so
            // one tenant never reads another tenant's cached value.
            if let Some(value) = self.cached_value(user_id, request.service, request.key) {
                self.audit_resolution(
                    db,
                    user_id,
                    agent,
                    request.service,
                    request.key,
                    request.mode,
                )
                .await?;
                return Ok(value);
            }
        }

        let document = self
            .fetch_secret_document(request.service, request.key)
            .await?;
        let secret = extract_secret_value(&document.value)?;

        if request.use_cache {
            // SECURITY (SEC-MED-1): cache inserts are scoped per tenant.
            self.store_cache(user_id, request.service, request.key, &secret);
        }

        self.audit_resolution(
            db,
            user_id,
            agent,
            &document.service,
            &document.key,
            request.mode,
        )
        .await?;
        Ok(secret)
    }

    async fn audit_resolution(
        &self,
        db: &Database,
        user_id: i64,
        agent: &str,
        service: &str,
        key: &str,
        mode: SecretAccessMode,
    ) -> Result<()> {
        let _ = log_action(
            db,
            LogActionRequest {
                agent: agent.to_string(),
                service: Some("cred".to_string()),
                action: "resolve".to_string(),
                narrative: None,
                payload: Some(json!({
                    "svc": service,
                    "key": key,
                    "agent": agent,
                    "tier": mode.tier(),
                })),
                axon_event_id: None,
                user_id: Some(user_id),
            },
        )
        .await?;
        Ok(())
    }

    async fn fetch_secret_document(&self, service: &str, key: &str) -> Result<CreddSecretDocument> {
        let url = self.build_url(&["secret", service, key])?;
        let response = self
            .http
            .get(url)
            .bearer_auth(self.agent_key()?)
            .send()
            .await
            .map_err(|e| EngError::Internal(format!("credd request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(EngError::Internal(format!(
                "credd request failed with status {}",
                response.status()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| EngError::Internal(format!("invalid credd response: {}", e)))
    }

    fn build_url(&self, segments: &[&str]) -> Result<Url> {
        let mut url = Url::parse(&self.base_url)
            .map_err(|e| EngError::InvalidInput(format!("invalid credd url: {}", e)))?;
        {
            let mut path_segments = url
                .path_segments_mut()
                .map_err(|_| EngError::InvalidInput("credd url cannot be a base".into()))?;
            path_segments.clear();
            for segment in segments {
                path_segments.push(segment);
            }
        }
        Ok(url)
    }

    pub(crate) fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.http.request(method, url)
    }

    fn agent_key(&self) -> Result<String> {
        if let Some(value) = &self.agent_key {
            return Ok(value.clone());
        }

        std::env::var(&self.agent_key_env).map_err(|_| {
            EngError::Auth(format!(
                "missing credd agent key env {}",
                self.agent_key_env
            ))
        })
    }

    fn cached_value(&self, user_id: i64, service: &str, key: &str) -> Option<String> {
        let cache = self.cache.lock().expect("credd cache mutex poisoned");
        let entry = cache.get(&cache_key(user_id, service, key))?;
        if entry.expires_at <= Instant::now() {
            return None;
        }
        Some(entry.value.clone())
    }

    fn store_cache(&self, user_id: i64, service: &str, key: &str, value: &str) {
        let mut cache = self.cache.lock().expect("credd cache mutex poisoned");
        cache.insert(
            cache_key(user_id, service, key),
            CachedSecret {
                value: value.to_string(),
                expires_at: Instant::now() + self.cache_ttl,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Environment export block extraction and trust evaluation
// (ported from eidolon-daemon/src/secrets.rs)
// ---------------------------------------------------------------------------

/// For an Environment-type secret, build a shell export block from all
/// key-value pairs in the secret value object.
///
/// Returns a string of the form `export K1='v1'; export K2='v2'; ` that
/// can be prepended to a shell command so the variables are available.
///
/// Returns an error if the secret is not of type `"Environment"`, or if
/// the value object is empty.
pub fn extract_env_export_block(secret: &Value) -> crate::Result<String> {
    let secret_type = secret
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if secret_type != "Environment" {
        return Err(crate::EngError::InvalidInput(format!(
            "env export block only valid for Environment type, got '{}'",
            secret_type
        )));
    }

    let val = secret
        .get("value")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            crate::EngError::InvalidInput(
                "Environment secret has no value object".to_string(),
            )
        })?;

    // Filter out the serde tag field "type" from the export block.
    let exports: Vec<String> = val
        .iter()
        .filter(|(k, _)| k.as_str() != "type")
        .filter_map(|(k, v)| {
            v.as_str().map(|val_str| {
                // Shell-escape the value using single-quote wrapping.
                let escaped = val_str.replace('\'', "'\\''");
                format!("export {}='{}'", k, escaped)
            })
        })
        .collect();

    if exports.is_empty() {
        return Err(crate::EngError::InvalidInput(
            "Environment secret has no key-value pairs".to_string(),
        ));
    }

    Ok(format!("{}; ", exports.join("; ")))
}

/// Trust evaluation seam. Currently returns 0 (deny-by-default).
/// Phase 2 will track session age, tool call count, and gate block count
/// to produce a decaying score.
#[allow(unused_variables)]
pub fn evaluate_trust(session_id: &str) -> u8 {
    0
}

/// Build the reqwest client used for every credd call.
///
/// SECURITY / ROBUSTNESS: every HTTP call to credd must time out. Without
/// an explicit timeout a hung credd (or a misconfigured URL pointing at a
/// port where nothing is listening but the kernel still holds the SYN)
/// can pin request handlers forever and exhaust tokio's task pool. The
/// scrub path in `sessions::scrub` catches errors and falls back to
/// unredacted text, so a timeout is a safe default.
fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Cache key shape: `{user_id}:{service}/{key}`. The `user_id` prefix is
/// what enforces tenant isolation (SEC-MED-1); the `:{service}/{key}`
/// suffix is stable so `invalidate_all_tenants` can match across tenants.
fn cache_key(user_id: i64, service: &str, key: &str) -> String {
    format!("{user_id}:{service}/{key}")
}

fn parse_secret_refs(payload: &Value) -> Result<Vec<(String, String)>> {
    let array = match payload {
        Value::Array(items) => items,
        Value::Object(map) => map
            .get("secrets")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                EngError::Internal("credd list response missing secrets array".into())
            })?,
        _ => {
            return Err(EngError::Internal(
                "credd list response has unsupported shape".into(),
            ))
        }
    };

    let mut refs = Vec::new();
    for item in array {
        let Some(service) = item.get("service").and_then(Value::as_str) else {
            continue;
        };
        let Some(key) = item.get("key").and_then(Value::as_str) else {
            continue;
        };
        refs.push((service.to_string(), key.to_string()));
    }
    Ok(refs)
}

fn extract_secret_value(value: &Value) -> Result<String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Object(map) => {
            for key in [
                "value",
                "key",
                "password",
                "token",
                "secret",
                "private_key",
                "content",
            ] {
                if let Some(text) = map.get(key).and_then(Value::as_str) {
                    return Ok(text.to_string());
                }
            }

            let non_type_fields: Vec<&Value> = map
                .iter()
                .filter(|(key, _)| key.as_str() != "type")
                .map(|(_, value)| value)
                .collect();
            if non_type_fields.len() == 1 {
                return match non_type_fields[0] {
                    Value::String(text) => Ok(text.clone()),
                    other => serde_json::to_string(other).map_err(EngError::Serialization),
                };
            }

            Err(EngError::Internal(
                "credd secret response did not contain a usable string value".into(),
            ))
        }
        other => serde_json::to_string(other).map_err(EngError::Serialization),
    }
}
