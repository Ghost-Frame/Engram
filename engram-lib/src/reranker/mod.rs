mod pool;
pub use pool::SessionPool;

use crate::config::Config;
use crate::embeddings::download::ensure_reranker_model;
use crate::{EngError, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Arc;
use tokenizers::Tokenizer;
use tracing::info;

/// Cross-encoder reranker using IBM Granite model with a pool of ONNX sessions.
pub struct Reranker {
    pool: SessionPool,
    tokenizer: Arc<Tokenizer>,
    max_seq: usize,
    top_k: usize,
}

impl Reranker {
    pub async fn new(config: &Config) -> Result<Self> {
        let model_dir = config.model_dir("granite-reranker");
        let (tokenizer_path, model_path) = ensure_reranker_model(&model_dir).await?;

        let tokenizer = Tokenizer::from_file(&tokenizer_path).map_err(|e| {
            EngError::Internal(format!(
                "failed to load reranker tokenizer from {}: {}",
                tokenizer_path.display(),
                e
            ))
        })?;

        let pool_size = config.reranker_pool_size.max(1);
        let mut sessions = Vec::with_capacity(pool_size);
        for i in 0..pool_size {
            let session = Session::builder()
                .map_err(|e| EngError::Internal(format!("ort session builder error: {}", e)))?
                .with_intra_threads(2)
                .map_err(|e| EngError::Internal(format!("ort thread config error: {}", e)))?
                .commit_from_file(&model_path)
                .map_err(|e| {
                    EngError::Internal(format!(
                        "failed to load reranker model from {}: {}",
                        model_path.display(),
                        e
                    ))
                })?;
            sessions.push(session);
            tracing::debug!(session = i, "reranker session created");
        }

        info!(
            model = %model_path.display(),
            pool_size = pool_size,
            top_k = config.reranker_top_k,
            "cross-encoder reranker pool initialized"
        );

        Ok(Self {
            pool: SessionPool::new(sessions),
            tokenizer: Arc::new(tokenizer),
            max_seq: 512,
            top_k: config.reranker_top_k,
        })
    }

    /// Rerank search results by cross-encoder relevance, scoring in parallel.
    pub async fn rerank_results(
        &self,
        query: &str,
        results: &mut [crate::memory::types::SearchResult],
    ) -> Result<()> {
        if results.is_empty() {
            return Ok(());
        }

        let k = self.top_k.min(results.len());
        let start = std::time::Instant::now();

        // Build parallel scoring tasks
        let tasks: Vec<_> = (0..k)
            .map(|i| {
                let doc = results[i].memory.content.clone();
                let q = query.to_string();
                let tokenizer = self.tokenizer.clone();
                let max_seq = self.max_seq;
                let pool = &self.pool;

                async move {
                    let mut pooled = pool.acquire().await?;
                    tokio::task::spawn_blocking(move || {
                        score_pair_inner(pooled.session_mut(), &tokenizer, &q, &doc, max_seq)
                    })
                    .await
                    .map_err(|e| EngError::Internal(format!("spawn_blocking join: {}", e)))?
                }
            })
            .collect();

        let scores: Vec<Result<f32>> = futures::future::join_all(tasks).await;

        for (i, score_result) in scores.into_iter().enumerate() {
            match score_result {
                Ok(ce_score) => {
                    // Blend: 70% cross-encoder, 30% original score
                    results[i].score = ce_score as f64 * 0.7 + results[i].score * 0.3;
                }
                Err(e) => {
                    tracing::warn!(idx = i, err = %e, "rerank score failed, keeping original");
                }
            }
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let elapsed = start.elapsed();
        tracing::info!(
            elapsed_ms = elapsed.as_millis(),
            k = k,
            pool_size = self.pool.size(),
            "parallel rerank complete"
        );

        Ok(())
    }
}

/// Score a single query-document pair using a locked session.
/// Returns relevance score 0-1 (sigmoid of logit).
fn score_pair_inner(
    session: &mut Session,
    tokenizer: &Tokenizer,
    query: &str,
    document: &str,
    max_seq: usize,
) -> Result<f32> {
    let encoding = tokenizer
        .encode((query, document), true)
        .map_err(|e| EngError::Internal(format!("reranker tokenization error: {}", e)))?;

    let token_ids = encoding.get_ids();
    let attention = encoding.get_attention_mask();
    let type_ids = encoding.get_type_ids();

    let mut input_ids = vec![0i64; max_seq];
    let mut attention_mask = vec![0i64; max_seq];
    let mut token_type_ids = vec![0i64; max_seq];

    let copy_len = token_ids.len().min(max_seq);
    for i in 0..copy_len {
        input_ids[i] = token_ids[i] as i64;
        attention_mask[i] = attention[i] as i64;
        token_type_ids[i] = type_ids[i] as i64;
    }

    let ids_tensor = Tensor::<i64>::from_array(([1usize, max_seq], input_ids))
        .map_err(|e| EngError::Internal(format!("failed to create input_ids tensor: {}", e)))?;
    let mask_tensor =
        Tensor::<i64>::from_array(([1usize, max_seq], attention_mask)).map_err(|e| {
            EngError::Internal(format!("failed to create attention_mask tensor: {}", e))
        })?;
    let type_ids_tensor =
        Tensor::<i64>::from_array(([1usize, max_seq], token_type_ids)).map_err(|e| {
            EngError::Internal(format!("failed to create token_type_ids tensor: {}", e))
        })?;

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
