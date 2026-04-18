use serde::{Deserialize, Serialize};

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
            model: "llama3.2:3b".into(),
            timeout_bg_ms: 60_000,
            timeout_hot_ms: 5_000,
            concurrency: 1,
            max_queue: 50,
            cb_threshold: 3,
            cb_cooldown_ms: 30_000,
        }
    }
}

impl OllamaConfig {
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Ok(v) = std::env::var("OLLAMA_URL") {
            cfg.url = v;
        }
        if let Ok(v) = std::env::var("OLLAMA_MODEL") {
            cfg.model = v;
        }
        if let Ok(v) = std::env::var("OLLAMA_TIMEOUT_BG_MS") {
            if let Ok(n) = v.parse() {
                cfg.timeout_bg_ms = n;
            }
        }
        if let Ok(v) = std::env::var("OLLAMA_TIMEOUT_HOT_MS") {
            if let Ok(n) = v.parse() {
                cfg.timeout_hot_ms = n;
            }
        }
        if let Ok(v) = std::env::var("OLLAMA_CONCURRENCY") {
            if let Ok(n) = v.parse() {
                cfg.concurrency = n;
            }
        }
        cfg
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

/// Circuit breaker state for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CircuitBreakerState {
    Closed,
    Open,
    HalfOpen,
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
