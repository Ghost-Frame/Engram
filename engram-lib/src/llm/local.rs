// ============================================================================
// LOCAL LLM CLIENT -- Ollama integration with semaphore and circuit breaker.
// Ported from TypeScript llm/local.ts
// ============================================================================

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::{EngError, Result};

// ============================================================================
// CONFIG
// ============================================================================

/// Configuration for the Ollama local model client.
#[derive(Debug, Clone)]
pub struct OllamaConfig {
    /// Ollama OpenAI-compatible endpoint URL.
    pub url: String,
    /// Default model name.
    pub model: String,
    /// Timeout for background (queued) requests in ms.
    pub timeout_bg_ms: u64,
    /// Timeout for hot-path (latency-critical) requests in ms.
    pub timeout_hot_ms: u64,
    /// Maximum concurrent requests to Ollama.
    pub concurrency: usize,
    /// Maximum queued requests before rejecting.
    pub max_queue: usize,
    /// Circuit breaker: consecutive failures before opening.
    pub cb_threshold: u32,
    /// Circuit breaker: cooldown in ms before half-open probe.
    pub cb_cooldown_ms: u64,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:11434/v1/chat/completions".into(),
            model: "qwen2.5:14b".into(),
            timeout_bg_ms: 60_000,
            timeout_hot_ms: 5_000,
            concurrency: 1,
            max_queue: 50,
            cb_threshold: 3,
            cb_cooldown_ms: 30_000,
        }
    }
}

/// Request priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Hot,
    Background,
}

/// Options for a single LLM call.
#[derive(Debug, Clone)]
pub struct CallOptions {
    pub model: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub timeout_ms: Option<u64>,
    pub priority: Priority,
}

impl Default for CallOptions {
    fn default() -> Self {
        Self {
            model: None,
            temperature: None,
            max_tokens: None,
            timeout_ms: None,
            priority: Priority::Background,
        }
    }
}

// ============================================================================
// CIRCUIT BREAKER
// ============================================================================

struct CircuitBreaker {
    failures: AtomicU32,
    open_until_ms: AtomicI64,
    threshold: u32,
    cooldown_ms: u64,
}

impl CircuitBreaker {
    fn new(threshold: u32, cooldown_ms: u64) -> Self {
        Self {
            failures: AtomicU32::new(0),
            open_until_ms: AtomicI64::new(0),
            threshold,
            cooldown_ms,
        }
    }

    fn is_open(&self) -> bool {
        let failures = self.failures.load(Ordering::Relaxed);
        if failures < self.threshold {
            return false;
        }
        let now_ms = now_epoch_ms();
        let open_until = self.open_until_ms.load(Ordering::Relaxed);
        if now_ms >= open_until {
            // Half-open: allow one probe
            self.failures.store(self.threshold - 1, Ordering::Relaxed);
            return false;
        }
        true
    }

    fn record_success(&self) {
        self.failures.store(0, Ordering::Relaxed);
        self.open_until_ms.store(0, Ordering::Relaxed);
    }

    fn record_failure(&self) {
        let prev = self.failures.fetch_add(1, Ordering::Relaxed);
        if prev + 1 >= self.threshold {
            let open_until = now_epoch_ms() + self.cooldown_ms as i64;
            self.open_until_ms.store(open_until, Ordering::Relaxed);
            tracing::warn!(
                msg = "ollama_circuit_open",
                cooldown_ms = self.cooldown_ms,
                failures = prev + 1,
            );
        }
    }

    fn state(&self) -> CircuitBreakerState {
        let failures = self.failures.load(Ordering::Relaxed);
        if failures < self.threshold {
            return CircuitBreakerState::Closed;
        }
        let now_ms = now_epoch_ms();
        let open_until = self.open_until_ms.load(Ordering::Relaxed);
        if now_ms >= open_until {
            CircuitBreakerState::HalfOpen
        } else {
            CircuitBreakerState::Open
        }
    }
}

/// Circuit breaker state for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

// ============================================================================
// LOCAL MODEL CLIENT
// ============================================================================

/// Ollama-based local LLM client with concurrency limiting and circuit breaker.
pub struct LocalModelClient {
    config: OllamaConfig,
    http: reqwest::Client,
    circuit_breaker: CircuitBreaker,
    semaphore: Arc<Semaphore>,
    queue_len: AtomicUsize,
    probe_result: AtomicU32, // 0=unknown, 1=ok, 2=failed
}

impl LocalModelClient {
    /// Create a new client with the given config.
    pub fn new(config: OllamaConfig) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.concurrency));
        let cb = CircuitBreaker::new(config.cb_threshold, config.cb_cooldown_ms);
        Self {
            http: reqwest::Client::new(),
            circuit_breaker: cb,
            semaphore,
            queue_len: AtomicUsize::new(0),
            probe_result: AtomicU32::new(0),
            config,
        }
    }

    /// Probe Ollama availability by hitting /api/tags.
    pub async fn probe(&self) -> bool {
        let tags_url = self.config.url
            .replace("/v1/chat/completions", "")
            .replace("/v1", "")
            + "/api/tags";

        let result = self.http
            .get(&tags_url)
            .timeout(Duration::from_secs(3))
            .send()
            .await;

        let ok = matches!(result, Ok(ref r) if r.status().is_success());
        self.probe_result.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
        tracing::info!(msg = "ollama_probe", reachable = ok, url = %self.config.url, model = %self.config.model);
        ok
    }

    /// Check if the local model is likely available.
    pub fn is_available(&self) -> bool {
        if self.circuit_breaker.is_open() { return false; }
        let probe = self.probe_result.load(Ordering::Relaxed);
        if probe == 2 { return false; }
        true
    }

    /// Call the local model with system + user prompts.
    pub async fn call(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        opts: Option<CallOptions>,
    ) -> Result<String> {
        let opts = opts.unwrap_or_default();
        let priority = opts.priority;
        let timeout_ms = opts.timeout_ms.unwrap_or(
            if priority == Priority::Hot { self.config.timeout_hot_ms } else { self.config.timeout_bg_ms }
        );
        let model = opts.model.as_deref().unwrap_or(&self.config.model);

        if self.circuit_breaker.is_open() {
            return Err(EngError::Internal("ollama circuit breaker open".into()));
        }

        // Semaphore: hot-path tries without waiting, background queues
        let permit = if priority == Priority::Hot {
            match self.semaphore.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => return Err(EngError::Internal("ollama busy (hot-path fast-fail)".into())),
            }
        } else {
            let queue = self.queue_len.fetch_add(1, Ordering::Relaxed);
            if queue >= self.config.max_queue {
                self.queue_len.fetch_sub(1, Ordering::Relaxed);
                return Err(EngError::Internal("ollama queue full".into()));
            }
            let permit = self.semaphore.clone().acquire_owned().await
                .map_err(|_| EngError::Internal("semaphore closed".into()))?;
            self.queue_len.fetch_sub(1, Ordering::Relaxed);
            permit
        };

        let body = serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_prompt },
            ],
            "temperature": opts.temperature.unwrap_or(0.1),
            "max_tokens": opts.max_tokens.unwrap_or(2000),
            "stream": false,
        });

        let result = self.http
            .post(&self.config.url)
            .header("Content-Type", "application/json")
            .timeout(Duration::from_millis(timeout_ms))
            .json(&body)
            .send()
            .await;

        drop(permit);

        match result {
            Ok(resp) => {
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body_text = resp.text().await.unwrap_or_default();
                    self.circuit_breaker.record_failure();
                    return Err(EngError::Internal(
                        format!("ollama {}: {}", status, &body_text[..body_text.len().min(200)])
                    ));
                }

                let data: serde_json::Value = resp.json().await
                    .map_err(|e| { self.circuit_breaker.record_failure(); EngError::Internal(format!("ollama json: {}", e)) })?;

                let text = data["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                if text.is_empty() {
                    self.circuit_breaker.record_failure();
                    return Err(EngError::Internal("ollama returned empty response".into()));
                }

                self.circuit_breaker.record_success();
                self.probe_result.store(1, Ordering::Relaxed);
                Ok(text)
            }
            Err(e) => {
                self.circuit_breaker.record_failure();
                Err(EngError::Internal(format!("ollama request failed: {}", e)))
            }
        }
    }

    /// Get stats for health/diagnostics endpoint.
    pub fn stats(&self) -> LocalModelStats {
        LocalModelStats {
            available: self.is_available(),
            circuit_breaker: self.circuit_breaker.state(),
            failures: self.circuit_breaker.failures.load(Ordering::Relaxed),
            semaphore_running: self.config.concurrency - self.semaphore.available_permits(),
            semaphore_queued: self.queue_len.load(Ordering::Relaxed),
            model: self.config.model.clone(),
            url: self.config.url.clone(),
        }
    }
}

/// Diagnostic stats for the local model client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelStats {
    pub available: bool,
    pub circuit_breaker: CircuitBreakerState,
    pub failures: u32,
    pub semaphore_running: usize,
    pub semaphore_queued: usize,
    pub model: String,
    pub url: String,
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let c = OllamaConfig::default();
        assert_eq!(c.url, "http://127.0.0.1:11434/v1/chat/completions");
        assert_eq!(c.model, "qwen2.5:14b");
        assert_eq!(c.timeout_bg_ms, 60_000);
        assert_eq!(c.timeout_hot_ms, 5_000);
        assert_eq!(c.concurrency, 1);
        assert_eq!(c.max_queue, 50);
        assert_eq!(c.cb_threshold, 3);
        assert_eq!(c.cb_cooldown_ms, 30_000);
    }

    #[test]
    fn test_circuit_breaker_closed() {
        let cb = CircuitBreaker::new(3, 30_000);
        assert!(!cb.is_open());
        assert_eq!(cb.state(), CircuitBreakerState::Closed);
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let cb = CircuitBreaker::new(3, 30_000);
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open()); // 2 < 3
        cb.record_failure();
        assert!(cb.is_open()); // 3 >= 3
        assert_eq!(cb.state(), CircuitBreakerState::Open);
    }

    #[test]
    fn test_circuit_breaker_resets_on_success() {
        let cb = CircuitBreaker::new(3, 30_000);
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_open());
        cb.record_success();
        assert!(!cb.is_open());
        assert_eq!(cb.state(), CircuitBreakerState::Closed);
    }

    #[test]
    fn test_circuit_breaker_half_open_after_cooldown() {
        let cb = CircuitBreaker::new(3, 0); // 0ms cooldown
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open()); // half-open allows probe
    }

    #[test]
    fn test_client_not_available_when_probe_failed() {
        let client = LocalModelClient::new(OllamaConfig::default());
        client.probe_result.store(2, Ordering::Relaxed);
        assert!(!client.is_available());
    }

    #[test]
    fn test_client_available_by_default() {
        let client = LocalModelClient::new(OllamaConfig::default());
        assert!(client.is_available());
    }

    #[test]
    fn test_stats_default() {
        let client = LocalModelClient::new(OllamaConfig::default());
        let s = client.stats();
        assert!(s.available);
        assert_eq!(s.circuit_breaker, CircuitBreakerState::Closed);
        assert_eq!(s.failures, 0);
        assert_eq!(s.semaphore_running, 0);
        assert_eq!(s.semaphore_queued, 0);
        assert_eq!(s.model, "qwen2.5:14b");
    }
}
