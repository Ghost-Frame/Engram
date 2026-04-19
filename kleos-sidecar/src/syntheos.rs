use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// SyntheosClient
//
// Thin async client for the four Syntheos sub-systems: Axon, Chiasm, Broca,
// and Soma. All methods are no-ops when `enabled` is false.
// All methods fire-and-forget: they spawn a tokio task internally and swallow
// errors at trace level so Syntheos flakes never block observation ingestion.
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SyntheosClient {
    http: reqwest::Client,
    base_url: String,
    token: Option<String>,
    pub enabled: bool,
    // Stored after successful Soma registration at startup; used for heartbeat.
    registered_agent_id: Arc<AtomicI64>,
}

impl SyntheosClient {
    /// Construct from the supplied params. Reads `ENGRAM_SIDECAR_SYNTHEOS` to
    /// decide whether integration is active (default off).
    pub fn new_from_env(http: reqwest::Client, base_url: String, token: Option<String>) -> Self {
        let enabled = std::env::var("ENGRAM_SIDECAR_SYNTHEOS")
            .map(|v| v.trim() == "1")
            .unwrap_or(false);
        Self {
            http,
            base_url,
            token,
            enabled,
            registered_agent_id: Arc::new(AtomicI64::new(-1)),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    fn apply_auth(token: &Option<String>, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match token {
            Some(t) => req.header("Authorization", format!("Bearer {}", t)),
            None => req,
        }
    }

    // -----------------------------------------------------------------------
    // Soma: registration + heartbeat
    // -----------------------------------------------------------------------

    /// Register the sidecar as a Soma agent at startup. Stores the returned
    /// agent id for subsequent heartbeat calls. Blocking (called once at startup).
    pub async fn register_soma_agent(
        &self,
        name: &str,
        agent_type: &str,
        capabilities: &[&str],
    ) -> Option<i64> {
        if !self.enabled {
            return None;
        }

        let url_str = self.url("/soma/agents");
        let url = match kleos_lib::net::validate_outbound_url(&url_str) {
            Ok(u) => u,
            Err(e) => {
                tracing::trace!(error = %e, "syntheos: soma register url invalid");
                return None;
            }
        };

        let body = json!({
            "name": name,
            "type": agent_type,
            "capabilities": capabilities,
        });

        let req = Self::apply_auth(&self.token, self.http.post(url).json(&body));

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                let val: Value = resp.json().await.unwrap_or_default();
                let id = val.get("id").and_then(|v| v.as_i64());
                if let Some(id) = id {
                    self.registered_agent_id.store(id, Ordering::Relaxed);
                    tracing::debug!(agent_id = id, "syntheos: soma agent registered");
                }
                id
            }
            Ok(resp) => {
                tracing::trace!(status = %resp.status(), "syntheos: soma register non-success");
                None
            }
            Err(e) => {
                tracing::trace!(error = %e, "syntheos: soma register failed");
                None
            }
        }
    }

    /// Fire-and-forget soma heartbeat. Pings the heartbeat endpoint and logs
    /// the status snapshot via the agent log endpoint.
    pub fn soma_heartbeat(&self, _agent_id: &str, status: Value) {
        if !self.enabled {
            return;
        }

        let id = self.registered_agent_id.load(Ordering::Relaxed);
        if id < 0 {
            return;
        }

        let http = self.http.clone();
        let token = self.token.clone();
        let base = self.base_url.clone();

        tokio::spawn(async move {
            let base = base.trim_end_matches('/');

            let hb_url = format!("{}/soma/agents/{}/heartbeat", base, id);
            if let Ok(url) = kleos_lib::net::validate_outbound_url(&hb_url) {
                let req = Self::apply_auth(&token, http.post(url));
                if let Err(e) = req.send().await {
                    tracing::trace!(error = %e, "syntheos: soma heartbeat ping failed");
                }
            }

            let log_url = format!("{}/soma/agents/{}/log", base, id);
            if let Ok(url) = kleos_lib::net::validate_outbound_url(&log_url) {
                let body = json!({ "level": "info", "message": "heartbeat", "data": status });
                let req = Self::apply_auth(&token, http.post(url).json(&body));
                if let Err(e) = req.send().await {
                    tracing::trace!(error = %e, "syntheos: soma heartbeat log failed");
                }
            }
        });
    }

    // -----------------------------------------------------------------------
    // Axon: publish event
    // -----------------------------------------------------------------------

    /// Publish an Axon event. POST /axon/publish. Fire-and-forget, 2 attempts.
    pub fn publish_axon(&self, channel: &str, action: &str, payload: Value) {
        if !self.enabled {
            return;
        }

        let http = self.http.clone();
        let token = self.token.clone();
        let url_str = self.url("/axon/publish");
        let body = json!({
            "channel": channel,
            "action": action,
            "payload": payload,
            "source": "kleos-sidecar",
            "agent": "kleos-sidecar",
        });

        tokio::spawn(async move {
            let result =
                kleos_lib::resilience::retry_with_backoff(2, Duration::from_millis(50), || {
                    let http = http.clone();
                    let body = body.clone();
                    let url_str = url_str.clone();
                    let token = token.clone();
                    async move {
                        let url = kleos_lib::net::validate_outbound_url(&url_str)
                            .map_err(|e| e.to_string())?;
                        let req = Self::apply_auth(&token, http.post(url).json(&body));
                        req.send().await.map(|_| ()).map_err(|e| e.to_string())
                    }
                })
                .await;
            if let Err(e) = result {
                tracing::trace!(error = %e, "syntheos: publish_axon failed after retries");
            }
        });
    }

    // -----------------------------------------------------------------------
    // Chiasm: task upsert
    // -----------------------------------------------------------------------

    /// Create a Chiasm task representing this session. POST /tasks.
    /// Uses session_id as the title so successive calls append task history
    /// without needing a stored task id (fire-and-forget constraint).
    pub fn upsert_chiasm_task(&self, session_id: &str, status: &str, summary: &str) {
        if !self.enabled {
            return;
        }

        let http = self.http.clone();
        let token = self.token.clone();
        let url_str = self.url("/tasks");
        let body = json!({
            "agent": "kleos-sidecar",
            "project": "kleos-sidecar",
            "title": session_id,
            "status": status,
            "summary": summary,
        });

        tokio::spawn(async move {
            let result =
                kleos_lib::resilience::retry_with_backoff(2, Duration::from_millis(50), || {
                    let http = http.clone();
                    let body = body.clone();
                    let url_str = url_str.clone();
                    let token = token.clone();
                    async move {
                        let url = kleos_lib::net::validate_outbound_url(&url_str)
                            .map_err(|e| e.to_string())?;
                        let req = Self::apply_auth(&token, http.post(url).json(&body));
                        req.send().await.map(|_| ()).map_err(|e| e.to_string())
                    }
                })
                .await;
            if let Err(e) = result {
                tracing::trace!(error = %e, "syntheos: upsert_chiasm_task failed after retries");
            }
        });
    }

    // -----------------------------------------------------------------------
    // Broca: action log
    // -----------------------------------------------------------------------

    /// Log a Broca action. POST /broca/actions. Fire-and-forget, 2 attempts.
    pub fn log_broca(&self, service: &str, action: &str, narrative: &str) {
        if !self.enabled {
            return;
        }

        let http = self.http.clone();
        let token = self.token.clone();
        let url_str = self.url("/broca/actions");
        let body = json!({
            "agent": "kleos-sidecar",
            "service": service,
            "action": action,
            "narrative": narrative,
        });

        tokio::spawn(async move {
            let result =
                kleos_lib::resilience::retry_with_backoff(2, Duration::from_millis(50), || {
                    let http = http.clone();
                    let body = body.clone();
                    let url_str = url_str.clone();
                    let token = token.clone();
                    async move {
                        let url = kleos_lib::net::validate_outbound_url(&url_str)
                            .map_err(|e| e.to_string())?;
                        let req = Self::apply_auth(&token, http.post(url).json(&body));
                        req.send().await.map(|_| ()).map_err(|e| e.to_string())
                    }
                })
                .await;
            if let Err(e) = result {
                tracing::trace!(error = %e, "syntheos: log_broca failed after retries");
            }
        });
    }
}
