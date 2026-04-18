use crate::embeddings::EmbeddingProvider;
use crate::{EngError, Result};

/// OpenAI embeddings API provider.
///
/// Sends requests to https://api.openai.com/v1/embeddings and returns
/// Vec<f32> embeddings. Validates that the returned dimension matches
/// the configured `dim`.
pub struct OpenAiProvider {
    http: reqwest::Client,
    api_key: String,
    model: String,
    dim: usize,
}

impl OpenAiProvider {
    /// Create a new OpenAI embedding provider.
    ///
    /// - `api_key`: OpenAI API key (Bearer token)
    /// - `model`: model name; defaults to "text-embedding-3-small" if None
    /// - `dim`: expected embedding dimension for validation
    pub fn new(http: reqwest::Client, api_key: String, model: Option<String>, dim: usize) -> Self {
        OpenAiProvider {
            http,
            api_key,
            model: model.unwrap_or_else(|| "text-embedding-3-small".to_string()),
            dim,
        }
    }
}

impl EmbeddingProvider for OpenAiProvider {
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<f32>>> + Send + 'a>> {
        use tracing::Instrument;
        let span = tracing::info_span!(
            "embed_openai",
            backend = "openai",
            model = %self.model,
            text_len = text.len(),
            dim = self.dim
        );
        Box::pin(async move {
            let url = "https://api.openai.com/v1/embeddings";
            let resp = self
                .http
                .post(url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&serde_json::json!({
                    "input": text,
                    "model": self.model,
                }))
                .send()
                .await
                .map_err(|e| EngError::Internal(format!("openai embed network: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(EngError::Internal(format!(
                    "openai returned {}: {}",
                    status, body
                )));
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| EngError::Internal(format!("openai parse: {}", e)))?;

            let embedding: Vec<f32> = body["data"][0]["embedding"]
                .as_array()
                .ok_or_else(|| EngError::Internal("openai: missing embedding array".to_string()))?
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();

            if embedding.len() != self.dim {
                return Err(EngError::Internal(format!(
                    "openai dimension mismatch: expected {}, got {}",
                    self.dim,
                    embedding.len()
                )));
            }

            Ok(embedding)
        }.instrument(span))
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Vec<f32>>>> + Send + 'a>>
    {
        use tracing::Instrument;
        let span = tracing::info_span!(
            "embed_openai_batch",
            backend = "openai",
            model = %self.model,
            batch_size = texts.len(),
            dim = self.dim
        );
        Box::pin(async move {
            let url = "https://api.openai.com/v1/embeddings";
            let resp = self
                .http
                .post(url)
                .header("Authorization", format!("Bearer {}", self.api_key))
                .json(&serde_json::json!({
                    "input": texts,
                    "model": self.model,
                }))
                .send()
                .await
                .map_err(|e| EngError::Internal(format!("openai batch network: {}", e)))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(EngError::Internal(format!(
                    "openai batch returned {}: {}",
                    status, body
                )));
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| EngError::Internal(format!("openai batch parse: {}", e)))?;

            let data = body["data"].as_array().ok_or_else(|| {
                EngError::Internal("openai batch: missing data array".to_string())
            })?;

            let mut results = Vec::with_capacity(data.len());
            for item in data {
                let embedding: Vec<f32> = item["embedding"]
                    .as_array()
                    .ok_or_else(|| {
                        EngError::Internal("openai batch: missing embedding field".to_string())
                    })?
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();

                if embedding.len() != self.dim {
                    return Err(EngError::Internal(format!(
                        "openai batch dimension mismatch: expected {}, got {}",
                        self.dim,
                        embedding.len()
                    )));
                }
                results.push(embedding);
            }

            Ok(results)
        }.instrument(span))
    }
}
