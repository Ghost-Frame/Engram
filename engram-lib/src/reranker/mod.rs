use crate::config::Config;
use crate::embeddings::download::ensure_reranker_model;
use crate::{EngError, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;
use tracing::info;

/// Cross-encoder reranker using IBM Granite model.
///
/// Heavy state lives behind an `Arc<RerankerInner>` so blocking work can be
/// scheduled on `spawn_blocking` with a cheap clone instead of a raw-pointer
/// cast, closing the previous use-after-free risk.
pub struct Reranker {
    inner: Arc<RerankerInner>,
    top_k: usize,
}

struct RerankerInner {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    max_seq: usize,
}

impl Reranker {
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
            "cross-encoder reranker initialized"
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

impl Reranker {
    /// Rerank search results by cross-encoder relevance.
    pub async fn rerank_results(
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
}
