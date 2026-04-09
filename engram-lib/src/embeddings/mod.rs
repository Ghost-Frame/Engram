pub mod chunking;
pub mod download;
pub mod normalize;
pub mod onnx;

use crate::Result;

pub trait EmbeddingProvider: Send + Sync {
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<f32>>> + Send + 'a>>;
}
