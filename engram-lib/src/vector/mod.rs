pub mod lance;

pub use lance::LanceIndex;

use crate::Result;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct VectorHit {
    pub memory_id: i64,
    pub distance: Option<f32>,
    pub rank: usize,
}

#[async_trait]
pub trait VectorIndex: Send + Sync {
    async fn insert(&self, memory_id: i64, user_id: i64, embedding: &[f32]) -> Result<()>;
    async fn search(&self, embedding: &[f32], limit: usize, user_id: i64)
        -> Result<Vec<VectorHit>>;
    async fn delete(&self, memory_id: i64) -> Result<()>;
    async fn count(&self) -> Result<usize>;
}
