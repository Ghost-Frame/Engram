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
    async fn insert(&self, memory_id: i64, embedding: &[f32]) -> Result<()>;
    async fn search(&self, embedding: &[f32], limit: usize) -> Result<Vec<VectorHit>>;
    async fn delete(&self, memory_id: i64) -> Result<()>;
    async fn count(&self) -> Result<usize>;

    /// Batched upsert. Default impl loops `insert`; LanceIndex overrides with
    /// a single delete+add round trip so a backfill pass paying the
    /// O(table_size) delete-scan once per call instead of once per row.
    async fn insert_many(&self, items: &[(i64, Vec<f32>)]) -> Result<()> {
        for (id, emb) in items {
            self.insert(*id, emb).await?;
        }
        Ok(())
    }

    /// Force a rebuild of the ANN index. `replace=true` drops any existing
    /// index before rebuilding. Returns `true` if the index was (re)built,
    /// `false` if the operation was skipped (below row threshold, or an
    /// existing index satisfied `replace=false`).
    async fn rebuild_index(&self, replace: bool) -> Result<bool>;
}
