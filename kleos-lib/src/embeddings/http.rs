use crate::embeddings::EmbeddingProvider;
use crate::{EngError, Result};

/// Generic HTTP embedding provider.
///
/// POSTs `{"text": "..."}` to the configured URL and expects a
/// `{"embedding": [...]}` response. Validates that the returned
/// dimension matches the configured `dim`.
///
/// Optionally sends an `Authorization` header for authenticated endpoints.
pub struct HttpProvider {
    http: reqwest::Client,
    url: String,
    dim: usize,
    auth_header: Option<String>,
}

impl HttpProvider {
    /// Create a new generic HTTP embedding provider.
    ///
    /// - `url`: full URL to POST embedding requests to
    /// - `dim`: expected embedding dimension for validation
    /// - `auth_header`: optional value for the `Authorization` header
    pub fn new(
        http: reqwest::Client,
        url: String,
        dim: usize,
        auth_header: Option<String>,
    ) -> Self {
        HttpProvider {
            http,
            url,
            dim,
            auth_header,
        }
    }
}

impl EmbeddingProvider for HttpProvider {
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<f32>>> + Send + 'a>> {
        use tracing::Instrument;
        let span = tracing::info_span!(
            "embed_http",
            backend = "http",
            text_len = text.len(),
            dim = self.dim
        );
        Box::pin(async move {
            let mut req = self
                .http
                .post(&self.url)
                .json(&serde_json::json!({"text": text}));

            if let Some(ref auth) = self.auth_header {
                req = req.header("Authorization", auth.as_str());
            }

            let resp = req
                .send()
                .await
                .map_err(|e| EngError::Internal(format!("http embed network: {}", e)))?;

            if !resp.status().is_success() {
                return Err(EngError::Internal(format!(
                    "http embed returned {}",
                    resp.status()
                )));
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| EngError::Internal(format!("http embed parse: {}", e)))?;

            let embedding: Vec<f32> =
                serde_json::from_value(body["embedding"].clone()).map_err(|e| {
                    EngError::Internal(format!("http embed: embedding field invalid: {}", e))
                })?;

            if embedding.len() != self.dim {
                return Err(EngError::Internal(format!(
                    "http embed dimension mismatch: expected {}, got {}",
                    self.dim,
                    embedding.len()
                )));
            }

            Ok(embedding)
        }.instrument(span))
    }
}
