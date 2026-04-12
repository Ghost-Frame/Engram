use crate::config::Config;
use crate::embeddings::chunking::chunk_text_with_limit;
use crate::embeddings::download::ensure_embedding_model;
use crate::embeddings::normalize::{l2_normalize, mean_pool, weighted_mean_pool};
use crate::embeddings::EmbeddingProvider;
use crate::{EngError, Result};
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;
use tracing::info;

/// Local ONNX-based embedding provider using bge-m3 (or compatible model).
///
/// The heavy state (ONNX session, tokenizer, model dimensions) is held behind
/// an `Arc` so that blocking work can be scheduled on `spawn_blocking` with a
/// cheap clone instead of a raw-pointer cast. This removes the previous
/// use-after-free risk if the provider was dropped mid-inference.
pub struct OnnxProvider {
    inner: Arc<OnnxInner>,
    chunk_max_chars: usize,
    chunk_overlap: usize,
    chunk_max_chunks: usize,
}

struct OnnxInner {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
    dim: usize,
    max_seq: usize,
}

impl OnnxProvider {
    /// Create a new OnnxProvider. Downloads model files if needed.
    pub async fn new(config: &Config) -> Result<Self> {
        let model_dir = config.model_dir("bge-m3");
        let (tokenizer_path, model_path) = ensure_embedding_model(
            &model_dir,
            &config.embedding_onnx_file,
            config.embedding_offline_only,
        )
        .await?;

        // Load tokenizer + ONNX session on a blocking thread so we do not
        // stall the tokio runtime during startup (S6-25).
        let dim = config.embedding_dim;
        let max_seq = config.embedding_max_seq;
        let chunk_max_chars = config.embedding_chunk_max_chars;
        let chunk_overlap = config.embedding_chunk_overlap;
        let chunk_max_chunks = config.embedding_chunk_max_chunks;

        tokio::task::spawn_blocking(move || {
            Self::from_files(
                &tokenizer_path,
                &model_path,
                dim,
                max_seq,
                chunk_max_chars,
                chunk_overlap,
                chunk_max_chunks,
            )
        })
        .await
        .map_err(|e| EngError::Internal(format!("onnx load task panicked: {}", e)))?
    }

    /// Create from explicit file paths (useful for testing).
    pub fn from_files(
        tokenizer_path: &Path,
        model_path: &Path,
        dim: usize,
        max_seq: usize,
        chunk_max_chars: usize,
        chunk_overlap: usize,
        chunk_max_chunks: usize,
    ) -> Result<Self> {
        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| {
            EngError::Internal(format!(
                "failed to load tokenizer from {}: {}",
                tokenizer_path.display(),
                e
            ))
        })?;

        // Build ONNX session using ort 2.0.0-rc.12 API.
        // with_intra_threads returns BuilderResult = Result<SessionBuilder, Error<SessionBuilder>>,
        // so we use map_err to unify the error type.
        let session = Session::builder()
            .map_err(|e| EngError::Internal(format!("ort session builder error: {}", e)))?
            .with_intra_threads(4)
            .map_err(|e| EngError::Internal(format!("ort thread config error: {}", e)))?
            .commit_from_file(model_path)
            .map_err(|e| {
                EngError::Internal(format!(
                    "failed to load ONNX model from {}: {}",
                    model_path.display(),
                    e
                ))
            })?;

        info!(
            model = %model_path.display(),
            dim = dim,
            max_seq = max_seq,
            "ONNX embedding provider initialized"
        );

        Ok(Self {
            inner: Arc::new(OnnxInner {
                session: Mutex::new(session),
                tokenizer,
                dim,
                max_seq,
            }),
            chunk_max_chars,
            chunk_overlap,
            chunk_max_chunks,
        })
    }
}

impl OnnxInner {
    /// Embed a single text string (no chunking).
    fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        // 1. Tokenize
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| EngError::Internal(format!("tokenization error: {}", e)))?;

        let token_ids = encoding.get_ids();
        let attention = encoding.get_attention_mask();

        // 2. Pad or truncate to max_seq
        let seq_len = self.max_seq;
        let mut input_ids = vec![0i64; seq_len];
        let mut attention_mask = vec![0i64; seq_len];

        let copy_len = token_ids.len().min(seq_len);
        for i in 0..copy_len {
            input_ids[i] = token_ids[i] as i64;
            attention_mask[i] = attention[i] as i64;
        }

        // Keep a copy of attention_mask for mean pooling before tensors consume input_ids/attention_mask
        let attention_mask_for_pool = attention_mask.clone();

        // 3. Build tensors using ort::value::Tensor::from_array with (shape, data) tuple.
        // Shape must be Vec<i64> or similar. Using [batch, seq_len] as [1usize, seq_len].
        let ids_tensor = Tensor::<i64>::from_array(([1usize, seq_len], input_ids))
            .map_err(|e| EngError::Internal(format!("failed to create input_ids tensor: {}", e)))?;
        let mask_tensor =
            Tensor::<i64>::from_array(([1usize, seq_len], attention_mask)).map_err(|e| {
                EngError::Internal(format!("failed to create attention_mask tensor: {}", e))
            })?;

        // 4. Run inference using named inputs macro.
        // ort::inputs! with "name" => value syntax returns a Vec (not a Result).
        let mut session = self
            .session
            .lock()
            .map_err(|e| EngError::Internal(format!("session mutex poisoned: {}", e)))?;

        let outputs = session
            .run(ort::inputs![
                "input_ids" => ids_tensor,
                "attention_mask" => mask_tensor,
            ])
            .map_err(|e| EngError::Internal(format!("ort inference error: {}", e)))?;

        // 5. Extract first output tensor as ArrayViewD<f32>
        let output_value = &outputs[0];
        let tensor_view = output_value
            .try_extract_array::<f32>()
            .map_err(|e| EngError::Internal(format!("failed to extract output tensor: {}", e)))?;

        let shape = tensor_view.shape();

        // 6. Handle 2D [1, dim] vs 3D [1, seq, dim] output
        let mut embedding = if shape.len() == 2 {
            // [1, dim] -- take the single row
            tensor_view
                .as_slice()
                .ok_or_else(|| EngError::Internal("output tensor not contiguous".to_string()))?
                .to_vec()
        } else if shape.len() == 3 {
            // [1, seq, dim] -- mean pool over non-padding positions
            let hidden = tensor_view
                .as_slice()
                .ok_or_else(|| EngError::Internal("output tensor not contiguous".to_string()))?;
            mean_pool(hidden, &attention_mask_for_pool, seq_len, self.dim)
        } else {
            return Err(EngError::Internal(format!(
                "unexpected output tensor shape: {:?}",
                shape
            )));
        };

        // 7. L2 normalize and truncate to dim
        l2_normalize(&mut embedding);
        embedding.truncate(self.dim);

        Ok(embedding)
    }
}

impl EmbeddingProvider for OnnxProvider {
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<f32>>> + Send + 'a>> {
        let chunk_max_chars = self.chunk_max_chars;
        let chunk_overlap = self.chunk_overlap;
        let chunk_max_chunks = self.chunk_max_chunks;
        let inner = Arc::clone(&self.inner);
        Box::pin(async move {
            let text = text.to_string();

            // Short text: embed directly on blocking thread. Cloning the Arc
            // keeps OnnxInner alive for the full duration of the blocking task,
            // so dropping the OnnxProvider mid-inference is safe.
            if text.len() <= chunk_max_chars {
                let inner_cloned = Arc::clone(&inner);
                let result = tokio::task::spawn_blocking(move || inner_cloned.embed_single(&text))
                    .await
                    .map_err(|e| EngError::Internal(format!("spawn_blocking join error: {}", e)))?;
                return result;
            }

            // Long text: chunk, embed each, weighted mean pool
            let chunks =
                chunk_text_with_limit(&text, chunk_max_chars, chunk_overlap, chunk_max_chunks);

            if chunks.is_empty() {
                return Err(EngError::InvalidInput(
                    "text produced no chunks after splitting".to_string(),
                ));
            }

            if chunks.len() == 1 {
                let chunk = chunks.into_iter().next().unwrap();
                let inner_cloned = Arc::clone(&inner);
                let result = tokio::task::spawn_blocking(move || inner_cloned.embed_single(&chunk))
                    .await
                    .map_err(|e| EngError::Internal(format!("spawn_blocking join error: {}", e)))?;
                return result;
            }

            let weights: Vec<f32> = chunks.iter().map(|c| c.len() as f32).collect();
            let mut embeddings = Vec::with_capacity(chunks.len());

            for chunk in chunks {
                let inner_cloned = Arc::clone(&inner);
                let emb = tokio::task::spawn_blocking(move || inner_cloned.embed_single(&chunk))
                    .await
                    .map_err(|e| {
                        EngError::Internal(format!("spawn_blocking join error: {}", e))
                    })??;
                embeddings.push(emb);
            }

            Ok(weighted_mean_pool(&embeddings, &weights))
        })
    }
}
