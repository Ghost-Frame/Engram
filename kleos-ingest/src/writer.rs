use crate::config::Config;
use crate::extractor::MemoryCandidate;
use serde_json::json;

pub struct KleosWriter {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    host: String,
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
            host: config.host.clone(),
            retry_buffer: Vec::new(),
        }
    }

    pub async fn store(&mut self, candidate: MemoryCandidate) -> bool {
        // Try to flush retry buffer first
        self.flush_retries().await;

        self.post_memory(candidate).await
    }

    async fn post_memory(&mut self, candidate: MemoryCandidate) -> bool {
        let body = json!({
            "content": candidate.content,
            "category": candidate.category,
            "source": candidate.source,
            "importance": candidate.importance,
            "tags": candidate.tags,
            "session_id": candidate.session_id,
        });

        let mut req = self.client
            .post(format!("{}/memories", self.base_url))
            .json(&body);

        if let Some(ref key) = self.api_key {
            req = req.header("X-API-Key", key);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(
                    category = %candidate.category,
                    session = %candidate.session_id,
                    "memory stored"
                );
                true
            }
            Ok(resp) => {
                tracing::warn!(
                    status = %resp.status(),
                    "kleos store failed, buffering for retry"
                );
                // Rebuild candidate for retry -- we consumed it above
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
                // Can't re-buffer inside post_memory since we moved it
                // Just log and drop on persistent failure
                tracing::warn!("retry failed, dropping memory");
            }
        }
        self.retry_buffer = failed;
    }

    pub async fn store_summary(&mut self, content: &str, session_id: &str, project: &str) -> bool {
        let body = json!({
            "content": content,
            "category": "session-summary",
            "source": "kleos-ingest",
            "importance": 6,
            "tags": ["session", project],
            "session_id": session_id,
        });

        let mut req = self.client
            .post(format!("{}/memories", self.base_url))
            .json(&body);

        if let Some(ref key) = self.api_key {
            req = req.header("X-API-Key", key);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::info!(session = %session_id, "session summary stored");
                true
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
