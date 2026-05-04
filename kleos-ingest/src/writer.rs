use crate::config::Config;
use crate::extractor::MemoryCandidate;
use serde_json::json;

pub struct KleosWriter {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    signer: Option<kleos_lib::auth_piv::RequestSigner>,
    signer_host: String,
    retry_buffer: Vec<MemoryCandidate>,
}

impl KleosWriter {
    pub fn new(config: &Config) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to build HTTP client");

        Self {
            client,
            base_url: config.kleos_url.clone(),
            api_key: config.api_key.clone(),
            signer: None,
            signer_host: config.host.clone(),
            retry_buffer: Vec::new(),
        }
    }

    fn ensure_signer(&mut self) -> Option<&kleos_lib::auth_piv::RequestSigner> {
        if self.signer.is_none() {
            match kleos_lib::auth_piv::RequestSigner::from_env_or_file(
                &self.signer_host,
                "kleos-ingest",
                "daemon",
            ) {
                Ok(Some(s)) => {
                    tracing::info!(tier = %s.tier(), fingerprint = %s.fingerprint(), "PIV/software key auth initialized (lazy)");
                    self.signer = Some(s);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(error = %e, "PIV/software key not yet available, will retry");
                }
            }
        }
        self.signer.as_ref()
    }

    fn apply_auth(
        &mut self,
        req: reqwest::RequestBuilder,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> reqwest::RequestBuilder {
        if let Some(signer) = self.ensure_signer() {
            if let Some(session) = signer.cached_session() {
                return req.header("X-Kleos-Session", session);
            }
            let (url_path, query) = match path.split_once('?') {
                Some((p, q)) => (p, q),
                None => (path, ""),
            };
            match signer.sign_request(method, url_path, query, body) {
                Ok(signed) => return signed.apply_headers(req),
                Err(e) => {
                    tracing::warn!(error = %e, "PIV signing failed, falling back to API key");
                }
            }
        }
        if let Some(key) = &self.api_key {
            return req.header("X-API-Key", key);
        }
        req
    }

    fn capture_session(&self, resp: &reqwest::Response) {
        if let Some(signer) = &self.signer {
            if let Some(token) = resp.headers().get("x-kleos-session-issued") {
                if let Ok(t) = token.to_str() {
                    signer.set_session(t.to_string());
                }
            }
        }
    }

    pub async fn store(&mut self, candidate: MemoryCandidate) -> bool {
        self.flush_retries().await;
        self.post_memory(candidate).await
    }

    async fn post_memory(&mut self, candidate: MemoryCandidate) -> bool {
        let body_value = json!({
            "content": candidate.content,
            "category": candidate.category,
            "source": candidate.source,
            "importance": candidate.importance,
            "tags": candidate.tags,
            "session_id": candidate.session_id,
        });

        let body_bytes = match serde_json::to_vec(&body_value) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize memory body");
                return false;
            }
        };

        let path = "/memories";
        let req = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header("Content-Type", "application/json")
            .body(body_bytes.clone());
        let req = self.apply_auth(req, "POST", path, &body_bytes);

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                self.capture_session(&resp);
                tracing::debug!(
                    category = %candidate.category,
                    session = %candidate.session_id,
                    "memory stored"
                );
                true
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                // If we were using a cached session token, clear it and retry with fresh signing
                let had_session = self
                    .signer
                    .as_ref()
                    .and_then(|s| s.cached_session())
                    .is_some();
                if had_session {
                    if let Some(signer) = &self.signer {
                        signer.clear_session();
                    }
                    tracing::debug!("session token expired (401), retrying with fresh PIV signing");
                    let req2 = self
                        .client
                        .post(format!("{}{}", self.base_url, path))
                        .header("Content-Type", "application/json")
                        .body(body_bytes.clone());
                    let req2 = self.apply_auth(req2, "POST", path, &body_bytes);
                    match req2.send().await {
                        Ok(resp2) if resp2.status().is_success() => {
                            self.capture_session(&resp2);
                            tracing::debug!(
                                category = %candidate.category,
                                session = %candidate.session_id,
                                "memory stored (after session refresh)"
                            );
                            return true;
                        }
                        Ok(resp2) => {
                            tracing::warn!(
                                status = %resp2.status(),
                                "kleos store failed after session refresh, buffering for retry"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "kleos unreachable after session refresh, buffering for retry");
                        }
                    }
                } else {
                    tracing::warn!(status = 401, "kleos store failed (unauthorized), buffering for retry");
                }
                false
            }
            Ok(resp) => {
                tracing::warn!(
                    status = %resp.status(),
                    "kleos store failed, buffering for retry"
                );
                false
            }
            Err(e) => {
                tracing::warn!(error = %e, "kleos unreachable, buffering for retry");
                false
            }
        }
    }

    async fn flush_retries(&mut self) {
        if self.retry_buffer.is_empty() {
            return;
        }
        let buffer = std::mem::take(&mut self.retry_buffer);
        let mut failed = Vec::new();
        for candidate in buffer {
            if !self.post_memory(candidate).await {
                tracing::warn!("retry failed, dropping memory");
            }
        }
        self.retry_buffer = failed;
    }

    pub async fn store_summary(&mut self, content: &str, session_id: &str, project: &str) -> bool {
        let body_value = json!({
            "content": content,
            "category": "session-summary",
            "source": "kleos-ingest",
            "importance": 6,
            "tags": ["session", project],
            "session_id": session_id,
        });

        let body_bytes = match serde_json::to_vec(&body_value) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize summary body");
                return false;
            }
        };

        let path = "/memories";
        let req = self
            .client
            .post(format!("{}{}", self.base_url, path))
            .header("Content-Type", "application/json")
            .body(body_bytes.clone());
        let req = self.apply_auth(req, "POST", path, &body_bytes);

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                self.capture_session(&resp);
                tracing::info!(session = %session_id, "session summary stored");
                true
            }
            Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
                let had_session = self
                    .signer
                    .as_ref()
                    .and_then(|s| s.cached_session())
                    .is_some();
                if had_session {
                    if let Some(signer) = &self.signer {
                        signer.clear_session();
                    }
                    tracing::debug!("session token expired (401), retrying summary with fresh PIV signing");
                    let req2 = self
                        .client
                        .post(format!("{}{}", self.base_url, path))
                        .header("Content-Type", "application/json")
                        .body(body_bytes.clone());
                    let req2 = self.apply_auth(req2, "POST", path, &body_bytes);
                    match req2.send().await {
                        Ok(resp2) if resp2.status().is_success() => {
                            self.capture_session(&resp2);
                            tracing::info!(session = %session_id, "session summary stored (after session refresh)");
                            return true;
                        }
                        Ok(resp2) => {
                            tracing::warn!(status = %resp2.status(), "summary store failed after session refresh");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "summary store failed after session refresh");
                        }
                    }
                } else {
                    tracing::warn!(status = 401, "summary store failed (unauthorized)");
                }
                false
            }
            Ok(resp) => {
                tracing::warn!(status = %resp.status(), "summary store failed");
                false
            }
            Err(e) => {
                tracing::warn!(error = %e, "summary store failed");
                false
            }
        }
    }
}
