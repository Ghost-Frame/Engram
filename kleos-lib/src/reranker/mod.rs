pub mod pool;
mod types;

use self::types::{
    CohereRerankRequest, CohereRerankResponse, RerankFormat, RerankResult, TeiRerankRequest,
    TeiRerankResponse,
};
use crate::config::Config;
use crate::db::Database;
use crate::embeddings::download::ensure_reranker_model;
use crate::resilience::ServiceGuard;
use crate::{EngError, Result};
use async_trait::async_trait;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;
use tokenizers::Tokenizer;
use tracing::{info, warn};

/// Default cross-encoder weight in the rerank blend: `final = ce*W + original*(1-W)`.
const DEFAULT_RERANKER_CE_WEIGHT: f64 = 0.7;

static RERANKER_CE_WEIGHT: LazyLock<f64> = LazyLock::new(|| {
    std::env::var("KLEOS_RERANKER_CE_WEIGHT")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .map(|w| w.clamp(0.0, 1.0))
        .unwrap_or(DEFAULT_RERANKER_CE_WEIGHT)
});

/// Runtime cross-encoder blend weight (KLEOS_RERANKER_CE_WEIGHT override, clamped to
/// [0, 1]). The original fusion score keeps the complementary weight `1 - w`, so recall
/// tuning can trade cross-encoder precision against fusion recall without a rebuild.
fn reranker_ce_weight() -> f64 {
    *RERANKER_CE_WEIGHT
}

// --- 3.13: Reranker trait -- swappable backends ---

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

    /// Number of top candidates this backend cross-encodes. Callers size the candidate
    /// pool to at least this many so the reranker can pull a buried-but-relevant memory
    /// up into the final top-k rather than only reordering an already-truncated set.
    fn top_k(&self) -> usize;
}

// --- ONNX cross-encoder backend (IBM Granite) ---

/// Cross-encoder reranker using IBM Granite model via ONNX Runtime.
pub struct OnnxReranker {
    inner: Arc<RerankerInner>,
    top_k: usize,
}

/// Shared, thread-safe inner state for the ONNX reranker (model session + tokenizer).
struct RerankerInner {
    /// ONNX Runtime session guarded by a mutex (the session is not Sync).
    session: Mutex<Session>,
    /// Tokenizer for the cross-encoder model.
    tokenizer: Tokenizer,
    /// Maximum sequence length fed to the model.
    max_seq: usize,
}

/// Constructor for the local ONNX cross-encoder reranker.
impl OnnxReranker {
    /// Load the Granite cross-encoder model and tokenizer and build the reranker.
    pub async fn new(config: &Config) -> Result<Self> {
        // Model name drives the on-disk dir and the download URL set.
        let model_name = config.reranker_model.clone();
        let model_dir = config.model_dir(&model_name);
        let (tokenizer_path, model_path) =
            ensure_reranker_model(&model_dir, &model_name, config.embedding_offline_only).await?;

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
            name = %model_name,
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

/// Cross-encoder scoring on the shared model session.
impl RerankerInner {
    /// Score a single query-document pair. Returns relevance score 0-1 (sigmoid of logit).
    fn score_pair(&self, query: &str, document: &str) -> Result<f32> {
        let encoding = self
            .tokenizer
            .encode((query, document), true)
            .map_err(|e| EngError::Internal(format!("reranker tokenization error: {}", e)))?;

        let token_ids = encoding.get_ids();
        let attention = encoding.get_attention_mask();

        let seq_len = self.max_seq;
        let mut input_ids = vec![0i64; seq_len];
        let mut attention_mask = vec![0i64; seq_len];

        let copy_len = token_ids.len().min(seq_len);
        for i in 0..copy_len {
            input_ids[i] = token_ids[i] as i64;
            attention_mask[i] = attention[i] as i64;
        }

        let ids_tensor = Tensor::<i64>::from_array(([1usize, seq_len], input_ids))
            .map_err(|e| EngError::Internal(format!("failed to create input_ids tensor: {}", e)))?;
        let mask_tensor =
            Tensor::<i64>::from_array(([1usize, seq_len], attention_mask)).map_err(|e| {
                EngError::Internal(format!("failed to create attention_mask tensor: {}", e))
            })?;

        let mut session = self
            .session
            .lock()
            .map_err(|e| EngError::Internal(format!("session mutex poisoned: {}", e)))?;

        let outputs = session
            .run(ort::inputs![
                "input_ids" => ids_tensor,
                "attention_mask" => mask_tensor,
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

/// Reranker implementation backed by the local ONNX cross-encoder.
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
    // Cross-encode the top candidates, blend with the fusion score, and re-sort.
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
            // Prefer the best-matching chunk over the full memory content: a long memory
            // truncated at the cross-encoder's 512-token window can hide the very passage
            // that matched the query. Fall back to full content for short/unchunked memories.
            let doc = result
                .matching_chunk
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| result.memory.content.clone());
            let q = query.to_string();
            let inner = Arc::clone(&self.inner);

            let ce_score = tokio::task::spawn_blocking(move || inner.score_pair(&q, &doc))
                .await
                .map_err(|e| EngError::Internal(format!("spawn_blocking join error: {}", e)))??;

            // L2 ABSTAIN: preserve the raw cross-encoder score (already a sigmoid of
            // the logit; see `score_pair`) BEFORE the fusion blend, so the abstain
            // gate reads an uncontaminated [0,1] confidence rather than the compound
            // `score` (which is entangled with decay/pagerank/recency).
            result.ce_confidence = Some(ce_score as f64);
            // Blend cross-encoder with the original fusion score (default 70/30,
            // KLEOS_RERANKER_CE_WEIGHT tunable).
            let w = reranker_ce_weight();
            result.score = ce_score as f64 * w + result.score * (1.0 - w);
            // Provenance: mark this row as cross-encoded so callers and the eval harness can
            // see the reranker actually ran (hybrid_search defaults reranked=false).
            result.reranked = Some(true);
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(())
    }

    /// Backend identifier for logs.
    fn backend_name(&self) -> &str {
        // Not Granite-specific anymore; the model is configurable.
        "onnx-cross-encoder"
    }

    /// Number of candidates this backend cross-encodes.
    fn top_k(&self) -> usize {
        self.top_k
    }
}

// --- HTTP reranker backend (Cohere / Jina compatible) ---

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
    format: RerankFormat,
    /// Unified resilience guard (circuit breaker + retry + dead-letter).
    /// `None` when constructed without a database (legacy path, no dead-lettering).
    guard: Option<Arc<ServiceGuard>>,
}

/// Constructors and request plumbing for the remote HTTP reranker.
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
        // R7-002: hardened builder (connect_timeout + redirect cap).
        let client = crate::net::safe_client_builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        let format = crate::kleos_env("RERANKER_FORMAT")
            .map(|s| RerankFormat::parse(&s))
            .unwrap_or_default();

        info!(
            endpoint = %endpoint,
            model = %model,
            format = ?format,
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
            format,
            guard,
        }
    }

    /// Construct from environment variables.
    ///
    /// Returns `None` when `KLEOS_RERANKER_URL` (or legacy `ENGRAM_RERANKER_HTTP_ENDPOINT`)
    /// is not set.
    ///
    /// Reads:
    ///   `KLEOS_RERANKER_URL`       — endpoint URL (required)
    ///   `KLEOS_RERANKER_API_KEY`   — Bearer token (optional)
    ///   `KLEOS_RERANKER_MODEL`     — model name (default: "rerank-v3.5", omitted for TEI)
    ///   `KLEOS_RERANKER_FORMAT`    — wire format: `cohere` (default) or `tei`
    ///   Legacy: `ENGRAM_RERANKER_HTTP_ENDPOINT`, `ENGRAM_RERANKER_HTTP_API_KEY`,
    ///           `ENGRAM_RERANKER_HTTP_MODEL`, `ENGRAM_RERANKER_FORMAT`
    pub fn from_env(top_k: usize, db: Option<Arc<Database>>) -> Option<Self> {
        let endpoint = std::env::var("KLEOS_RERANKER_URL")
            .or_else(|_| crate::kleos_env("RERANKER_HTTP_ENDPOINT"))
            .ok()?;
        let api_key = std::env::var("KLEOS_RERANKER_API_KEY")
            .or_else(|_| crate::kleos_env("RERANKER_HTTP_API_KEY"))
            .ok();
        let model = std::env::var("KLEOS_RERANKER_MODEL")
            .or_else(|_| crate::kleos_env("RERANKER_HTTP_MODEL"))
            .unwrap_or_else(|_| "rerank-v3.5".to_string());
        Some(Self::new_with_db(endpoint, api_key, model, top_k, db))
    }

    /// Configured endpoint URL.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Configured wire format.
    pub fn format(&self) -> RerankFormat {
        self.format
    }
}

/// Reranker implementation backed by a remote Cohere/Jina/TEI rerank API.
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
    // Call the remote rerank endpoint, blend the returned scores, and re-sort.
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
            // Prefer the best-matching chunk over full content (see the ONNX backend).
            .map(|r| {
                r.matching_chunk
                    .clone()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| r.memory.content.clone())
            })
            .collect();

        let format = self.format;

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
                    let mut req = match format {
                        RerankFormat::Tei => {
                            let body = TeiRerankRequest {
                                query: &query_s,
                                texts: doc_refs,
                                truncate: true,
                            };
                            client.post(&endpoint).json(&body)
                        }
                        RerankFormat::Cohere => {
                            let body = CohereRerankRequest {
                                model: &model,
                                query: &query_s,
                                documents: doc_refs,
                                top_n: documents.len(),
                            };
                            client.post(&endpoint).json(&body)
                        }
                    };
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
                    let results: Vec<RerankResult> = match format {
                        RerankFormat::Tei => resp
                            .json::<TeiRerankResponse>()
                            .await
                            .map_err(|e| {
                                EngError::Internal(format!(
                                    "HTTP reranker (TEI) response parse error: {}",
                                    e
                                ))
                            })?
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                        RerankFormat::Cohere => resp
                            .json::<CohereRerankResponse>()
                            .await
                            .map_err(|e| {
                                EngError::Internal(format!(
                                    "HTTP reranker (Cohere) response parse error: {}",
                                    e
                                ))
                            })?
                            .results
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                    };
                    Ok(results)
                }
            }
        };

        // When a ServiceGuard is available, route through it for full
        // circuit-breaker + retry + dead-letter coverage. Otherwise use a
        // simple inline retry loop (no dead-lettering).
        let rerank_resp: Vec<RerankResult> = if let Some(ref guard) = self.guard {
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
            let mut resp_result: Option<Vec<RerankResult>> = None;
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

        // Apply scores: blend remote score with the original (default 70/30,
        // KLEOS_RERANKER_CE_WEIGHT tunable).
        let w = reranker_ce_weight();
        for item in &rerank_resp {
            if item.index < results.len() {
                // L2 ABSTAIN: capture a [0,1]-normalized cross-encoder confidence
                // BEFORE the blend. Cohere returns a relevance score already in
                // range; TEI returns a raw logit, so map it through a sigmoid so a
                // single abstain threshold is comparable across backends. (If a TEI
                // deployment pre-normalizes its scores, recalibrate the threshold --
                // mandatory after any backend/model change regardless.)
                let ce_norm = match self.format {
                    RerankFormat::Tei => 1.0 / (1.0 + (-item.score).exp()),
                    RerankFormat::Cohere => item.score,
                };
                results[item.index].ce_confidence = Some(ce_norm);
                results[item.index].score = item.score * w + results[item.index].score * (1.0 - w);
                // Provenance: mark this row as reranked (see the ONNX backend).
                results[item.index].reranked = Some(true);
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(())
    }

    /// Backend identifier for logs (varies by wire format).
    fn backend_name(&self) -> &str {
        match self.format {
            RerankFormat::Tei => "http-tei",
            RerankFormat::Cohere => "http-cohere",
        }
    }

    /// Number of candidates this backend reranks.
    fn top_k(&self) -> usize {
        self.top_k
    }
}

// --- Factory: create reranker from config ---

/// Create the appropriate reranker backend based on config.
///
/// Backend selection (canonical `KLEOS_*` vars; legacy `ENGRAM_*` accepted as fallback):
/// - `KLEOS_RERANKER_BACKEND=onnx` (default): local ONNX cross-encoder (IBM Granite)
/// - `KLEOS_RERANKER_BACKEND=http`: remote HTTP API (Cohere/Jina or TEI)
/// - `KLEOS_RERANKER_BACKEND=none` or `reranker_enabled=false`: disabled
///
/// For HTTP backend, set:
/// - `KLEOS_RERANKER_URL`      (required; legacy: `ENGRAM_RERANKER_HTTP_ENDPOINT`)
/// - `KLEOS_RERANKER_API_KEY`  (optional; legacy: `ENGRAM_RERANKER_HTTP_API_KEY`)
/// - `KLEOS_RERANKER_MODEL`    (default: "rerank-v3.5"; legacy: `ENGRAM_RERANKER_HTTP_MODEL`)
/// - `KLEOS_RERANKER_FORMAT`   (`cohere` (default) or `tei`; legacy: `ENGRAM_RERANKER_FORMAT`)
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

    let backend = crate::kleos_env("RERANKER_BACKEND")
        .unwrap_or_else(|_| "onnx".to_string())
        .to_lowercase();

    match backend.as_str() {
        "onnx" | "local" => {
            let reranker = OnnxReranker::new(config).await?;
            Ok(Some(Arc::new(reranker) as Arc<dyn Reranker>))
        }
        "http" | "remote" | "cohere" | "jina" | "tei" => {
            let reranker = HttpReranker::from_env(config.reranker_top_k, db).ok_or_else(|| {
                EngError::InvalidInput(
                    "KLEOS_RERANKER_URL required for http reranker backend".into(),
                )
            })?;
            Ok(Some(Arc::new(reranker) as Arc<dyn Reranker>))
        }
        "none" | "disabled" => Ok(None),
        other => Err(EngError::InvalidInput(format!(
            "unknown reranker backend '{}'; expected onnx, http, tei, or none",
            other
        ))),
    }
}
