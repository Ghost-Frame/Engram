use super::{VectorHit, VectorIndex};
use crate::{EngError, Result};
use arrow_array::types::Float32Type;
use arrow_array::{Array, FixedSizeListArray, Float32Array, Int64Array, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use futures::TryStreamExt;
use lancedb::index::vector::IvfHnswPqIndexBuilder;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::DistanceType;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

const TABLE_NAME: &str = "memory_vectors";
const VECTOR_COLUMN: &str = "vector";

/// Minimum number of rows before an IVF_HNSW_PQ index is built. Below this
/// threshold LanceDB's IVF clustering does not converge cleanly and a linear
/// scan over the fixed-size-list is faster anyway, so we skip index creation.
pub const MIN_ROWS_FOR_INDEX: usize = 256;

fn lance_err(context: &str, err: impl std::fmt::Display) -> EngError {
    EngError::Internal(format!("{}: {}", context, err))
}

fn vector_schema(dimensions: usize) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("memory_id", DataType::Int64, false),
        Field::new("user_id", DataType::Int64, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimensions as i32,
            ),
            false,
        ),
    ]))
}

fn single_embedding_batch(
    memory_id: i64,
    user_id: i64,
    embedding: &[f32],
    schema: SchemaRef,
    dimensions: usize,
) -> Result<RecordBatch> {
    if embedding.len() != dimensions {
        return Err(EngError::InvalidInput(format!(
            "embedding dimension mismatch: expected {}, got {}",
            dimensions,
            embedding.len()
        )));
    }

    let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        [Some(
            embedding.iter().copied().map(Some).collect::<Vec<_>>(),
        )],
        dimensions as i32,
    );

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(Int64Array::from(vec![memory_id])),
            Arc::new(Int64Array::from(vec![user_id])),
            Arc::new(vectors),
        ],
    )
    .map_err(|e| lance_err("build LanceDB record batch", e))
}

pub struct LanceIndex {
    db: lancedb::Connection,
    table: RwLock<Option<lancedb::Table>>,
    dimensions: usize,
}

impl LanceIndex {
    pub async fn open(path: impl AsRef<str>, dimensions: usize) -> Result<Self> {
        let db = lancedb::connect(path.as_ref())
            .execute()
            .await
            .map_err(|e| lance_err("connect LanceDB", e))?;

        let index = Self {
            db,
            table: RwLock::new(None),
            dimensions,
        };
        let _ = index.ensure_table().await?;
        Ok(index)
    }

    async fn ensure_table(&self) -> Result<lancedb::Table> {
        if let Some(table) = self.table.read().await.as_ref().cloned() {
            return Ok(table);
        }

        let mut table_guard = self.table.write().await;
        if let Some(table) = table_guard.as_ref().cloned() {
            return Ok(table);
        }

        let table_names = self
            .db
            .table_names()
            .execute()
            .await
            .map_err(|e| lance_err("list LanceDB tables", e))?;

        let table = if table_names.iter().any(|name| name == TABLE_NAME) {
            self.db
                .open_table(TABLE_NAME)
                .execute()
                .await
                .map_err(|e| lance_err("open LanceDB vector table", e))?
        } else {
            self.db
                .create_empty_table(TABLE_NAME, vector_schema(self.dimensions))
                .execute()
                .await
                .map_err(|e| lance_err("create LanceDB vector table", e))?
        };

        *table_guard = Some(table.clone());
        Ok(table)
    }

    async fn ensure_vector_index(&self, table: &lancedb::Table) {
        let row_count = table.count_rows(None).await.unwrap_or(0);
        if row_count < MIN_ROWS_FOR_INDEX {
            return;
        }

        if self.vector_index_exists(table).await {
            return;
        }

        if let Err(e) = self.create_hnsw_index(table).await {
            warn!("LanceDB vector index create skipped: {}", e);
        }
    }

    async fn vector_index_exists(&self, table: &lancedb::Table) -> bool {
        table
            .list_indices()
            .await
            .map(|indices| {
                indices
                    .iter()
                    .any(|index| index.columns == vec![VECTOR_COLUMN.to_string()])
            })
            .unwrap_or(false)
    }

    async fn create_hnsw_index(&self, table: &lancedb::Table) -> Result<()> {
        let builder = IvfHnswPqIndexBuilder::default().distance_type(DistanceType::Cosine);
        table
            .create_index(&[VECTOR_COLUMN], Index::IvfHnswPq(builder))
            .execute()
            .await
            .map_err(|e| lance_err("create LanceDB IVF_HNSW_PQ index", e))
    }

    async fn drop_vector_index(&self, table: &lancedb::Table) -> Result<()> {
        let indices = table
            .list_indices()
            .await
            .map_err(|e| lance_err("list LanceDB indices", e))?;
        for index in indices {
            if index.columns == vec![VECTOR_COLUMN.to_string()] {
                table
                    .drop_index(&index.name)
                    .await
                    .map_err(|e| lance_err("drop LanceDB vector index", e))?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl VectorIndex for LanceIndex {
    async fn insert(&self, memory_id: i64, user_id: i64, embedding: &[f32]) -> Result<()> {
        let table = self.ensure_table().await?;
        let schema = vector_schema(self.dimensions);
        let batch = single_embedding_batch(
            memory_id,
            user_id,
            embedding,
            schema.clone(),
            self.dimensions,
        )?;

        table
            .delete(&format!("memory_id = {}", memory_id))
            .await
            .map_err(|e| lance_err("delete previous LanceDB vector row", e))?;
        table
            .add(vec![batch])
            .execute()
            .await
            .map_err(|e| lance_err("insert LanceDB vector row", e))?;

        self.ensure_vector_index(&table).await;
        Ok(())
    }

    async fn search(
        &self,
        embedding: &[f32],
        limit: usize,
        user_id: i64,
    ) -> Result<Vec<VectorHit>> {
        if embedding.len() != self.dimensions {
            return Err(EngError::InvalidInput(format!(
                "embedding dimension mismatch: expected {}, got {}",
                self.dimensions,
                embedding.len()
            )));
        }

        let table = self.ensure_table().await?;
        let batches: Vec<RecordBatch> = table
            .query()
            .nearest_to(embedding)
            .map_err(|e| lance_err("create LanceDB vector query", e))?
            .only_if(format!("user_id = {}", user_id))
            .limit(limit)
            .execute()
            .await
            .map_err(|e| lance_err("execute LanceDB vector query", e))?
            .try_collect()
            .await
            .map_err(|e| lance_err("collect LanceDB vector results", e))?;

        let mut hits = Vec::new();
        for batch in batches {
            let memory_idx = batch
                .schema()
                .index_of("memory_id")
                .map_err(|e| lance_err("read LanceDB memory_id column", e))?;
            let memory_ids = batch
                .column(memory_idx)
                .as_any()
                .downcast_ref::<Int64Array>()
                .ok_or_else(|| {
                    EngError::Internal("LanceDB memory_id column is not Int64".into())
                })?;

            let distance_values = batch
                .schema()
                .index_of("_distance")
                .ok()
                .and_then(|idx| batch.column(idx).as_any().downcast_ref::<Float32Array>());

            for row in 0..batch.num_rows() {
                let distance = distance_values.and_then(|dist| {
                    if dist.is_null(row) {
                        None
                    } else {
                        Some(dist.value(row))
                    }
                });
                hits.push(VectorHit {
                    memory_id: memory_ids.value(row),
                    distance,
                    rank: hits.len(),
                });
            }
        }

        Ok(hits)
    }

    async fn delete(&self, memory_id: i64) -> Result<()> {
        let table = self.ensure_table().await?;
        table
            .delete(&format!("memory_id = {}", memory_id))
            .await
            .map_err(|e| lance_err("delete LanceDB vector row", e))?;
        Ok(())
    }

    async fn count(&self) -> Result<usize> {
        let table = self.ensure_table().await?;
        table
            .count_rows(None)
            .await
            .map_err(|e| lance_err("count LanceDB vector rows", e))
    }

    async fn rebuild_index(&self, replace: bool) -> Result<bool> {
        let table = self.ensure_table().await?;
        let row_count = table
            .count_rows(None)
            .await
            .map_err(|e| lance_err("count LanceDB vector rows", e))?;
        if row_count < MIN_ROWS_FOR_INDEX {
            return Ok(false);
        }

        let exists = self.vector_index_exists(&table).await;
        if exists && !replace {
            return Ok(false);
        }
        if exists {
            self.drop_vector_index(&table).await?;
        }

        self.create_hnsw_index(&table).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temp_path() -> String {
        let dir = std::env::temp_dir().join(format!("engram-lance-{}", Uuid::new_v4()));
        dir.to_string_lossy().to_string()
    }

    #[tokio::test]
    async fn rebuild_index_skipped_below_row_threshold() {
        let path = temp_path();
        let index = LanceIndex::open(&path, 4).await.expect("open lance");
        index
            .insert(1, 1, &[0.1, 0.2, 0.3, 0.4])
            .await
            .expect("insert");
        index
            .insert(2, 1, &[0.9, 0.8, 0.7, 0.6])
            .await
            .expect("insert");
        assert_eq!(index.count().await.expect("count"), 2);

        let rebuilt = index.rebuild_index(true).await.expect("rebuild");
        assert!(
            !rebuilt,
            "small table must skip IVF_HNSW_PQ build (row_count < {})",
            MIN_ROWS_FOR_INDEX
        );

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn insert_then_search_linear_scan_returns_nearest() {
        let path = temp_path();
        let index = LanceIndex::open(&path, 4).await.expect("open lance");
        index
            .insert(42, 7, &[1.0, 0.0, 0.0, 0.0])
            .await
            .expect("insert");
        index
            .insert(43, 7, &[0.0, 1.0, 0.0, 0.0])
            .await
            .expect("insert");

        let hits = index
            .search(&[1.0, 0.0, 0.0, 0.0], 1, 7)
            .await
            .expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].memory_id, 42);

        let _ = std::fs::remove_dir_all(&path);
    }

    #[tokio::test]
    async fn rebuild_index_noop_when_table_empty() {
        let path = temp_path();
        let index = LanceIndex::open(&path, 4).await.expect("open lance");
        assert_eq!(index.count().await.expect("count"), 0);
        let rebuilt = index.rebuild_index(false).await.expect("rebuild");
        assert!(!rebuilt);

        let _ = std::fs::remove_dir_all(&path);
    }
}
