pub mod chunking;
pub mod normalize;

use crate::Result;

pub trait EmbeddingProvider: Send + Sync {
    fn embed<'a>(
        &'a self,
        text: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<f32>>> + Send + 'a>>;
}
