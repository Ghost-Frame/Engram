pub mod pool;
mod types;

use self::types::{HttpRerankRequest, HttpRerankResponse};
use crate::config::Config;
use crate::db::Database;
use crate::embeddings::download::ensure_reranker_model;
use crate::resilience::ServiceGuard;
use crate::{EngError, Result};
use async_trait::async_trait;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokenizers::Tokenizer;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// 3.13: Reranker trait -- swappable backends
// ---------------------------------------------------------------------------

/// Trait for reranking search results. Backends implement this to provide
/// different reranking strategies (local ONNX, remote HTTP API, noop).
#[async_trait]
pub trait Reranker: Send + Sync {
    /// Rerank the given search results in-place by adjusting scores.
    /// Only the first `top_k` results (configured per-backend) are reranked;
    /// the rest keep their original scores. Results are re-sorted after scoring.
    async fn rerank_results(
        &self,
        query: &str,
        results: &mut [crate::memory::types::SearchResult],
    ) -> Result<()>;

    /// Human-readable name for logging/diagnostics.
    fn backend_name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// ONNX cross-encoder backend (IBM Granite)
// ---------------------------------------------------------------------------

/// Cross-encoder reranker using IBM Granite model via ONNX Runtime.
pub struct OnnxReranker {
    inner: Arc<RerankerInner>,
    top_k: usize,
}

struct RerankerInner {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    max_seq: usize,
}

impl OnnxReranker {
    pub async fn new(config: &Config) -> Result<Self> {
        let model_dir = config.model_dir("granite-reranker");
        let (tokenizer_path, model_path) =
            ensure_reranker_model(&model_dir, config.embedding_offline_only).await?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            EngError::Internal(format!(
                "failed to load reranker tokenizer from {}: {}",
                tokenizer_path.display(),
                e
            ))
        })?;

        let session = Session::builder()
            .map_err(|e| EngError::Internal(format!("ort session builder error: {}", e)))?
            .with_intra_threads(4)
            .map_err(|e| EngError::Internal(format!("ort thread config error: {}", e)))?
            .commit_from_file(&model_path)
            .map_err(|e| {
                EngError::Internal(format!(
                    "failed to load reranker model from {}: {}",
                    model_path.display(),
                    e
                ))
            })?;

        info!(
            model = %model_path.display(),
            top_k = config.reranker_top_k,
            "ONNX cross-encoder reranker initialized"
        );

        Ok(Self {
            inner: Arc::new(RerankerInner {
                session: Mutex::new(session),
                tokenizer,
                max_seq: 512,
            }),
            top_k: config.reranker_top_k,
        })
    }
}

impl RerankerInner {
    /// Score a single query-document pair. Returns relevance score 0-1 (sigmoid of logit).
    fn score_pair(&self, query: &str, document: &str) -> Result<f32> {
        let encoding = self
            .tokenizer
            .encode((query, document), true)
            .map_err(|e| EngError::Internal(format!("reranker tokenization error: {}", e)))?;

        let token_ids = encoding.get_ids();
        let attention = encoding.get_attention_mask();
        let type_ids = encoding.get_type_ids();

        let seq_len = self.max_seq;
        let mut input_ids = vec![0i64; seq_len];
        let mut attention_mask = vec![0i64; seq_len];
        let mut token_type_ids = vec![0i64; seq_len];

        let copy_len = token_ids.len().min(seq_len);
        for i in 0..copy_len {
            input_ids[i] = token_ids[i] as i64;
            attention_mask[i] = attention[i] as i64;
            token_type_ids[i] = type_ids[i] as i64;
        }

        let ids_tensor = Tensor::<i64>::from_array(([1usize, seq_len], input_ids))
            .map_err(|e| EngError::Internal(format!("failed to create input_ids tensor: {}", e)))?;
        let mask_tensor =
            Tensor::<i64>::from_array(([1usize, seq_len], attention_mask)).map_err(|e| {
                EngError::Internal(format!("failed to create attention_mask tensor: {}", e))
            })?;
        let type_ids_tensor = Tensor::<i64>::from_array(([1usize, seq_len], token_type_ids))
            .map_err(|e| {
                EngError::Internal(format!("failed to create token_type_ids tensor: {}", e))
            })?;

        let mut session = self
            .session
            .lock()
            .map_err(|e| EngError::Internal(format!("session mutex poisoned: {}", e)))?;

        let outputs = session
            .run(ort::inputs![
                "input_ids" => ids_tensor,
                "attention_mask" => mask_tensor,
                "token_type_ids" => type_ids_tensor,
            ])
            .map_err(|e| EngError::Internal(format!("ort reranker inference error: {}", e)))?;

        let output_value = &outputs[0];
        let tensor_view = output_value
            .try_extract_array::<f32>()
            .map_err(|e| EngError::Internal(format!("failed to extract reranker output: {}", e)))?;

        let logit = tensor_view
            .as_slice()
            .and_then(|s| s.first())
            .copied()
            .unwrap_or(0.0);

        let score = 1.0 / (1.0 + (-logit).exp());
        Ok(score)
    }
}

#[async_trait]
impl Reranker for OnnxReranker {
    #[tracing::instrument(
        name = "rerank_onnx",
        skip_all,
        fields(
            backend = "onnx",
            query_len = query.len(),
            result_count = results.len(),
        )
    )]
    async fn rerank_results(
        &self,
        query: &str,
        results: &mut [crate::memory::types::SearchResult],
    ) -> Result<()> {
        if results.is_empty() {
            return Ok(());
        }

        let k = self.top_k.min(results.len());

        for result in results.iter_mut().take(k) {
            let doc = result.memory.content.clone();
            let q = query.to_string();
            let inner = Arc::clone(&self.inner);

            let ce_score = tokio::task::spawn_blocking(move || inner.score_pair(&q, &doc))
                .await
                .map_err(|e| EngError::Internal(format!("spawn_blocking join error: {}", e)))??;

            // Blend: 70% cross-encoder, 30% original score
            result.score = ce_score as f64 * 0.7 + result.score * 0.3;
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(())
    }

    fn backend_name(&self) -> &str {
        "onnx-granite"
    }
}

// ---------------------------------------------------------------------------
// HTTP reranker backend (Cohere / Jina compatible)
// ---------------------------------------------------------------------------

/// Remote HTTP reranker that calls a Cohere-compatible /v1/rerank API.
/// Also works with Jina Reranker (same API shape).
///
/// Expected response format:
/// ```json
/// { "results": [ { "index": 0, "relevance_score": 0.95 }, ... ] }
/// ```
///
/// Uses a [`ServiceGuard`] for circuit-breaker, retry, and dead-letter
/// support when a database is supplied via [`HttpReranker::new_with_db`].
pub struct HttpReranker {
    client: reqwest::Client,
    endpoint: String,
    api_key: Option<String>,
    model: String,
    top_k: usize,
    /// Unified resilience guard (circuit breaker + retry + dead-letter).
    /// `None` when constructed without a database (legacy path, no dead-lettering).
    guard: Option<Arc<ServiceGuard>>,
}

impl HttpReranker {
    /// Construct without a database. Uses an inline retry loop with no
    /// dead-letter recording. Use [`HttpReranker::new_with_db`] for full
    /// resilience coverage.
    pub fn new(endpoint: String, api_key: Option<String>, model: String, top_k: usize) -> Self {
        Self::new_with_db(endpoint, api_key, model, top_k, None)
    }

    /// Construct with a database for circuit-breaker + retry + dead-letter
    /// support via [`ServiceGuard`].
    pub fn new_with_db(
        endpoint: String,
        api_key: Option<String>,
        model: String,
        top_k: usize,
        db: Option<Arc<Database>>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        info!(
            endpoint = %endpoint,
            model = %model,
            top_k = top_k,
            has_guard = db.is_some(),
            "HTTP reranker configured"
        );

        let guard = db.map(|database| Arc::new(ServiceGuard::with_defaults("reranker", database)));

        Self {
            client,
            endpoint,
            api_key,
            model,
            top_k,
            guard,
        }
    }

    /// Current circuit state string for metrics/health checks.
    /// Returns "closed", "open", or "half_open". Returns "closed" when no
    /// guard is present (constructed without a database).
    pub fn breaker_state(&self) -> &'static str {
        match self.guard.as_ref().map(|g| g.circuit_state()) {
            Some(crate::resilience::CircuitState::Open) => "open",
            Some(crate::resilience::CircuitState::HalfOpen) => "half_open",
            _ => "closed",
        }
    }
}

#[async_trait]
impl Reranker for HttpReranker {
    #[tracing::instrument(
        name = "rerank_http",
        skip_all,
        fields(
            backend = "http",
            query_len = query.len(),
            result_count = results.len(),
        )
    )]
    async fn rerank_results(
        &self,
        query: &str,
        results: &mut [crate::memory::types::SearchResult],
    ) -> Result<()> {
        if results.is_empty() {
            return Ok(());
        }

        let k = self.top_k.min(results.len());

        // Build owned copies for the closure (must be Fn + Send + Sync).
        let client = self.client.clone();
        let endpoint = self.endpoint.clone();
        let api_key = self.api_key.clone();
        let model = self.model.clone();
        let query_s = query.to_string();
        let documents: Vec<String> = results
            .iter()
            .take(k)
            .map(|r| r.memory.content.clone())
            .collect();

        let http_call = {
            let client = client.clone();
            let endpoint = endpoint.clone();
            let api_key = api_key.clone();
            let model = model.clone();
            let query_s = query_s.clone();
            let documents = documents.clone();

            move || {
                let client = client.clone();
                let endpoint = endpoint.clone();
                let api_key = api_key.clone();
                let model = model.clone();
                let query_s = query_s.clone();
                let documents = documents.clone();

                async move {
                    let doc_refs: Vec<&str> = documents.iter().map(|s| s.as_str()).collect();
                    let body = HttpRerankRequest {
                        model: &model,
                        query: &query_s,
                        documents: doc_refs,
                        top_n: documents.len(),
                    };
                    let mut req = client.post(&endpoint).json(&body);
                    if let Some(ref key) = api_key {
                        req = req.header("Authorization", format!("Bearer {}", key));
                    }
                    let resp = req.send().await.map_err(|e| {
                        EngError::Internal(format!("HTTP reranker request failed: {}", e))
                    })?;
                    let status = resp.status();
                    if !status.is_success() {
                        let body_text = resp.text().await.unwrap_or_default();
                        return Err(EngError::Internal(format!(
                            "HTTP reranker returned {}: {}",
                            status, body_text
                        )));
                    }
                    resp.json::<HttpRerankResponse>().await.map_err(|e| {
                        EngError::Internal(format!("HTTP reranker response parse error: {}", e))
                    })
                }
            }
        };

        // When a ServiceGuard is available, route through it for full
        // circuit-breaker + retry + dead-letter coverage. Otherwise use a
        // simple inline retry loop (no dead-lettering).
        let rerank_resp: HttpRerankResponse = if let Some(ref guard) = self.guard {
            let payload = serde_json::json!({
                "query": query_s,
                "document_count": documents.len(),
            });
            match guard.call("rerank", payload, http_call).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(
                        error = %e,
                        "HTTP reranker ServiceGuard failed; returning base scores"
                    );
                    return Ok(());
                }
            }
        } else {
            // Legacy inline retry: 3 attempts, 200 ms base, no dead-letter.
            let mut last_err: Option<EngError> = None;
            let mut resp_result: Option<HttpRerankResponse> = None;
            for attempt in 0..3u32 {
                if attempt > 0 {
                    let delay_ms = 200u64.saturating_mul(1u64 << (attempt - 1));
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                match http_call().await {
                    Ok(r) => {
                        resp_result = Some(r);
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                    }
                }
            }
            match resp_result {
                Some(r) => r,
                None => {
                    let e = last_err.unwrap_or_else(|| {
                        EngError::Internal("HTTP reranker: all retries failed".into())
                    });
                    warn!(error = %e, "HTTP reranker retries exhausted; returning base scores");
                    return Ok(());
                }
            }
        };

        // Apply scores: blend 70% remote score, 30% original
        for item in &rerank_resp.results {
            if item.index < results.len() {
                results[item.index].score =
                    item.relevance_score * 0.7 + results[item.index].score * 0.3;
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(())
    }

    fn backend_name(&self) -> &str {
        "http"
    }
}

// ---------------------------------------------------------------------------
// Factory: create reranker from config
// ---------------------------------------------------------------------------

/// Create the appropriate reranker backend based on config.
///
/// Backend selection:
/// - `ENGRAM_RERANKER_BACKEND=onnx` (default): local ONNX cross-encoder
/// - `ENGRAM_RERANKER_BACKEND=http`: remote Cohere/Jina API
/// - `ENGRAM_RERANKER_BACKEND=none` or `reranker_enabled=false`: returns None
///
/// For HTTP backend, set:
/// - `ENGRAM_RERANKER_HTTP_ENDPOINT` (required, e.g. https://api.cohere.ai/v1/rerank)
/// - `ENGRAM_RERANKER_HTTP_API_KEY` (optional, for authenticated APIs)
/// - `ENGRAM_RERANKER_HTTP_MODEL` (default: "rerank-v3.5")
///
/// Pass `db` to enable dead-letter recording for the HTTP backend. When `None`
/// the HTTP backend uses the inline retry path without dead-lettering.
#[tracing::instrument(skip(config, db), fields(enabled = config.reranker_enabled))]
pub async fn create_reranker(
    config: &Config,
    db: Option<Arc<Database>>,
) -> Result<Option<Arc<dyn Reranker>>> {
    if !config.reranker_enabled {
        return Ok(None);
    }

    let backend = std::env::var("ENGRAM_RERANKER_BACKEND")
        .unwrap_or_else(|_| "onnx".to_string())
        .to_lowercase();

    match backend.as_str() {
        "onnx" | "local" => {
            let reranker = OnnxReranker::new(config).await?;
            Ok(Some(Arc::new(reranker) as Arc<dyn Reranker>))
        }
        "http" | "remote" | "cohere" | "jina" => {
            let endpoint = std::env::var("ENGRAM_RERANKER_HTTP_ENDPOINT").map_err(|_| {
                EngError::InvalidInput(
                    "ENGRAM_RERANKER_HTTP_ENDPOINT required for http reranker backend".into(),
                )
            })?;
            let api_key = std::env::var("ENGRAM_RERANKER_HTTP_API_KEY").ok();
            let model = std::env::var("ENGRAM_RERANKER_HTTP_MODEL")
                .unwrap_or_else(|_| "rerank-v3.5".to_string());
            let reranker =
                HttpReranker::new_with_db(endpoint, api_key, model, config.reranker_top_k, db);
            Ok(Some(Arc::new(reranker) as Arc<dyn Reranker>))
        }
        "none" | "disabled" => Ok(None),
        other => Err(EngError::InvalidInput(format!(
            "unknown reranker backend '{}'; expected onnx, http, or none",
            other
        ))),
    }
}
