mod types;

/// Re-exported so callers can select the HTTP wire format explicitly (see
/// [`HttpReranker::with_format`]).
pub use self::types::RerankFormat;
use self::types::{
    CohereRerankRequest, CohereRerankResponse, RerankResult, TeiRerankRequest, TeiRerankResponse,
};
use crate::config::Config;
use crate::db::Database;
#[cfg(feature = "ml")]
use crate::embeddings::download::ensure_reranker_model;
use crate::resilience::ServiceGuard;
use crate::{EngError, Result};
use async_trait::async_trait;
#[cfg(feature = "ml")]
use ort::session::Session;
#[cfg(feature = "ml")]
use ort::value::Tensor;
#[cfg(feature = "ml")]
use std::path::{Path, PathBuf};
#[cfg(feature = "ml")]
use std::sync::Mutex;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
#[cfg(feature = "ml")]
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
#[cfg(feature = "ml")]
pub struct OnnxReranker {
    inner: Arc<RerankerInner>,
    top_k: usize,
}

/// Shared, thread-safe inner state for the ONNX reranker (model session + tokenizer).
#[cfg(feature = "ml")]
struct RerankerInner {
    /// ONNX Runtime session guarded by a mutex (the session is not Sync).
    session: Mutex<Session>,
    /// Tokenizer for the cross-encoder model.
    tokenizer: Tokenizer,
    /// Maximum sequence length fed to the model.
    max_seq: usize,
}

/// A staged reranker model discovered on disk: its directory and short name.
#[cfg(feature = "ml")]
struct StagedReranker {
    /// Directory holding `tokenizer.json` + `model_quantized.onnx`.
    dir: PathBuf,
    /// Short model name (the directory's file name), used for logging.
    name: String,
}

/// Look beside `configured_dir` for another already-staged cross-encoder model.
///
/// Guards against the silent-disable regression where changing the DEFAULT reranker
/// (e.g. granite-reranker -> bge-reranker-v2-m3) breaks reranking on an offline box that
/// only pre-staged the previous model: instead of disabling reranking, fall back to a
/// sibling reranker that is actually present. Only directories whose name marks them a
/// reranker (`reranker`/`granite`) and that hold BOTH model files qualify, so the embedder
/// dir (bge-m3) is never mistaken for a cross-encoder. Returns the lowest-named match for
/// determinism, or `None` when nothing usable is staged.
#[cfg(feature = "ml")]
fn find_staged_reranker_fallback(
    configured_dir: &Path,
    configured_name: &str,
) -> Option<StagedReranker> {
    let root = configured_dir.parent()?;
    let mut candidates: Vec<StagedReranker> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let dir = entry.path();
            let name = dir.file_name()?.to_str()?.to_string();
            let looks_like_reranker = name.contains("reranker") || name.contains("granite");
            let staged =
                dir.join("tokenizer.json").is_file() && dir.join("model_quantized.onnx").is_file();
            if dir.is_dir() && name != configured_name && looks_like_reranker && staged {
                Some(StagedReranker { dir, name })
            } else {
                None
            }
        })
        .collect();
    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    candidates.into_iter().next()
}

/// Constructor for the local ONNX cross-encoder reranker.
#[cfg(feature = "ml")]
impl OnnxReranker {
    /// Load the Granite cross-encoder model and tokenizer and build the reranker.
    pub async fn new(config: &Config) -> Result<Self> {
        // Model name drives the on-disk dir and the download URL set.
        let mut model_name = config.reranker_model.clone();
        let model_dir = config.model_dir(&model_name);
        let (tokenizer_path, model_path) = match ensure_reranker_model(
            &model_dir,
            &model_name,
            config.embedding_offline_only,
        )
        .await
        {
            Ok(paths) => paths,
            // Offline boxes cannot download a newly-defaulted model. Rather than silently
            // disable reranking, substitute any other staged reranker so recall stays reranked.
            Err(e) if config.embedding_offline_only => {
                match find_staged_reranker_fallback(&model_dir, &model_name) {
                    Some(fallback) => {
                        warn!(
                            configured = %model_name,
                            fallback = %fallback.name,
                            "configured reranker '{}' is not staged for offline use; falling back \
                             to staged reranker '{}'. Recall stays reranked, but the abstain gate's \
                             thresholds were calibrated for '{}' -- recalibrate (or stage the \
                             configured model) if this fallback persists.",
                            model_name, fallback.name, model_name
                        );
                        let paths = ensure_reranker_model(
                            &fallback.dir,
                            &fallback.name,
                            config.embedding_offline_only,
                        )
                        .await?;
                        model_name = fallback.name;
                        paths
                    }
                    None => return Err(e),
                }
            }
            Err(e) => return Err(e),
        };

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
#[cfg(feature = "ml")]
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
#[cfg(feature = "ml")]
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

        // Normalize the fusion scores across the reranked window onto [0,1] before
        // blending. The fusion score is RRF-scale (~0.006-0.05) while the CE score is
        // a [0,1] sigmoid, so blending them raw leaves the (1-w) fusion weight
        // negligible and the reranked order effectively pure cross-encoder. Min-max
        // scaling restores the intended contribution of the fusion signal.
        let (fusion_min, fusion_max) = results
            .iter()
            .take(k)
            .map(|r| r.score)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), s| {
                (lo.min(s), hi.max(s))
            });
        let fusion_span = fusion_max - fusion_min;

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
            // Blend cross-encoder with the normalized fusion score (default 70/30,
            // KLEOS_RERANKER_CE_WEIGHT tunable). With span 0 (single candidate or
            // all-equal fusion) the fusion term is a neutral 0.5 so ordering falls to
            // the cross-encoder.
            let w = reranker_ce_weight();
            let norm_fusion = if fusion_span > 0.0 {
                (result.score - fusion_min) / fusion_span
            } else {
                0.5
            };
            result.score = ce_score as f64 * w + norm_fusion * (1.0 - w);
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

/// Default HTTP reranker request timeout in seconds (upstream behaviour).
const DEFAULT_RERANKER_TIMEOUT_SECS: u64 = 10;
/// Lower clamp for the HTTP reranker request timeout.
const MIN_RERANKER_TIMEOUT_SECS: u64 = 1;
/// Upper clamp for the HTTP reranker request timeout.
const MAX_RERANKER_TIMEOUT_SECS: u64 = 300;

/// Resolve the HTTP reranker request timeout (seconds) from an optional raw
/// env value (`KLEOS_RERANKER_TIMEOUT_SECS`, legacy `ENGRAM_RERANKER_TIMEOUT_SECS`).
///
/// Parses a `u64`, clamps to `[MIN_RERANKER_TIMEOUT_SECS, MAX_RERANKER_TIMEOUT_SECS]`,
/// and falls back to `DEFAULT_RERANKER_TIMEOUT_SECS` when the value is absent or
/// non-numeric (warning on the latter). Absent value reproduces the previous
/// hardcoded default exactly, so the behaviour is unchanged when the env var is unset.
fn resolve_reranker_timeout_secs(raw: Option<String>) -> u64 {
    match raw {
        None => DEFAULT_RERANKER_TIMEOUT_SECS,
        Some(v) => match v.parse::<u64>() {
            Ok(n) => n.clamp(MIN_RERANKER_TIMEOUT_SECS, MAX_RERANKER_TIMEOUT_SECS),
            Err(_) => {
                tracing::warn!(
                    "invalid env KLEOS_RERANKER_TIMEOUT_SECS={}, using default {}",
                    v,
                    DEFAULT_RERANKER_TIMEOUT_SECS
                );
                DEFAULT_RERANKER_TIMEOUT_SECS
            }
        },
    }
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
        // Request timeout is env-overridable (default 10s preserves prior behaviour).
        let timeout_secs =
            resolve_reranker_timeout_secs(crate::kleos_env("RERANKER_TIMEOUT_SECS").ok());

        // R7-002: hardened builder (connect_timeout + redirect cap).
        let client = crate::net::safe_client_builder()
            .timeout(Duration::from_secs(timeout_secs))
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
            timeout_secs = timeout_secs,
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
    ///   `KLEOS_RERANKER_TIMEOUT_SECS` — request timeout in seconds, clamped to [1, 300] (default 10)
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

    /// Override the wire format detected from the environment. Lets callers
    /// (and tests) select TEI vs Cohere explicitly instead of mutating the
    /// process-global `KLEOS_RERANKER_FORMAT` env var.
    pub fn with_format(mut self, format: RerankFormat) -> Self {
        self.format = format;
        self
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
        // Normalize the fusion scores across the reranked set onto [0,1] before
        // blending (see the ONNX backend): the fusion score is RRF-scale while the
        // remote CE score is [0,1], so without this the (1-w) fusion weight is
        // negligible and reranked order is effectively pure cross-encoder.
        let (fusion_min, fusion_max) = rerank_resp
            .iter()
            .filter(|item| item.index < results.len())
            .map(|item| results[item.index].score)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), s| {
                (lo.min(s), hi.max(s))
            });
        let fusion_span = fusion_max - fusion_min;
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
                // Normalized fusion contribution; neutral 0.5 when the span is zero.
                let norm_fusion = if fusion_span > 0.0 {
                    (results[item.index].score - fusion_min) / fusion_span
                } else {
                    0.5
                };
                // Blend with the [0,1]-normalized CE confidence, not the raw wire
                // score: a TEI logit is unbounded and would both swamp the (1-w)
                // fusion term and push blended scores outside [0,1] (breaking
                // min-relevance gating downstream). For Cohere, ce_norm IS the
                // wire score, so this is an identity change for that arm.
                results[item.index].score = ce_norm * w + norm_fusion * (1.0 - w);
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
        #[cfg(feature = "ml")]
        "onnx" | "local" => {
            let reranker = OnnxReranker::new(config).await?;
            Ok(Some(Arc::new(reranker) as Arc<dyn Reranker>))
        }
        // ml-off builds carry no local cross-encoder: fail with actionable
        // guidance instead of a missing-model error. Callers already treat
        // create_reranker errors as "run without a reranker".
        #[cfg(not(feature = "ml"))]
        "onnx" | "local" => Err(EngError::InvalidInput(
            "the onnx reranker backend requires a build with the 'ml' feature; \
             rebuild with default features, or set KLEOS_RERANKER_BACKEND=http \
             or none"
                .into(),
        )),
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

#[cfg(all(test, feature = "ml"))]
/// Tests for the offline staged-reranker fallback resolution (an ml-only
/// concern: the fallback exists to keep the local ONNX backend alive).
mod tests {
    use super::*;

    /// Create `root/<name>/` containing both reranker model files (content irrelevant).
    fn stage_model(root: &Path, name: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("tokenizer.json"), b"{}").unwrap();
        std::fs::write(dir.join("model_quantized.onnx"), b"onnx").unwrap();
    }

    /// A missing configured model falls back to a staged sibling reranker, never the embedder.
    #[test]
    fn fallback_picks_staged_sibling_reranker_not_embedder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        stage_model(root, "bge-m3"); // embedder -- must be ignored
        stage_model(root, "granite-reranker"); // staged previous reranker
                                               // Configured model dir is absent on disk.
        let configured = root.join("bge-reranker-v2-m3");

        let fb = find_staged_reranker_fallback(&configured, "bge-reranker-v2-m3")
            .expect("a staged reranker sibling should be found");
        assert_eq!(fb.name, "granite-reranker");
    }

    /// With no other reranker staged, the resolver returns None (caller keeps the hard error).
    #[test]
    fn fallback_none_when_only_embedder_present() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        stage_model(root, "bge-m3"); // only the embedder is present
        let configured = root.join("bge-reranker-v2-m3");

        assert!(find_staged_reranker_fallback(&configured, "bge-reranker-v2-m3").is_none());
    }

    /// A reranker dir missing one of the two model files does not qualify as staged.
    #[test]
    fn fallback_skips_incomplete_reranker_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // tokenizer present but the onnx file is absent.
        let partial = root.join("granite-reranker");
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join("tokenizer.json"), b"{}").unwrap();
        let configured = root.join("bge-reranker-v2-m3");

        assert!(find_staged_reranker_fallback(&configured, "bge-reranker-v2-m3").is_none());
    }

    /// Absent env value reproduces the previous hardcoded default of 10s.
    #[test]
    fn reranker_timeout_defaults_to_10_when_unset() {
        assert_eq!(resolve_reranker_timeout_secs(None), 10);
    }

    /// A valid value is honoured verbatim.
    #[test]
    fn reranker_timeout_honours_valid_value() {
        assert_eq!(resolve_reranker_timeout_secs(Some("30".to_string())), 30);
    }

    /// A non-numeric value falls back to the default.
    #[test]
    fn reranker_timeout_invalid_falls_back_to_default() {
        assert_eq!(resolve_reranker_timeout_secs(Some("abc".to_string())), 10);
    }

    /// Out-of-range values are clamped to [1, 300].
    #[test]
    fn reranker_timeout_clamps_out_of_range() {
        assert_eq!(resolve_reranker_timeout_secs(Some("0".to_string())), 1);
        assert_eq!(resolve_reranker_timeout_secs(Some("500".to_string())), 300);
    }
}
