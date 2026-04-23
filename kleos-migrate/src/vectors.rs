use anyhow::Result;
use arrow_array::types::Float32Type;
use arrow_array::{FixedSizeListArray, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader};
use arrow_schema::{DataType, Field, Schema};
use lancedb::index::Index;
use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

use crate::source::{self, SourceDb};

const TABLE_NAME: &str = "memory_vectors";
const DIMENSIONS: usize = 1024;

pub struct LanceDb {
    pub db: lancedb::Connection,
}

/// Open or create LanceDB at `<target_dir>/hnsw/memories.lance/`.
pub async fn open_lance(target_dir: &Path) -> Result<LanceDb> {
    let lance_path = target_dir.join("hnsw").join("memories.lance");
    std::fs::create_dir_all(&lance_path)?;

    info!("Opening LanceDB at {:?}", lance_path);
    let db = lancedb::connect(lance_path.to_string_lossy().as_ref())
        .execute()
        .await?;

    Ok(LanceDb { db })
}

fn vector_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("memory_id", DataType::Int64, false),
        Field::new("user_id", DataType::Int64, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                DIMENSIONS as i32,
            ),
            false,
        ),
    ]))
}

/// Extract embeddings from source memories and write them into LanceDB.
pub async fn extract_and_insert(
    source: &SourceDb,
    lance: &LanceDb,
    filter_user_id: i64,
) -> Result<()> {
    info!("Extracting memory vectors...");

    let source_cols = source::get_columns(source, "memories")?;
    let source_has_user_id = source_cols.iter().any(|c| c == "user_id");
    let source_has_vec_col = source_cols.iter().any(|c| c == "embedding_vec_1024");

    if !source_has_vec_col {
        info!("source memories table has no embedding_vec_1024 column -- skipping vector extraction");
        return Ok(());
    }

    let select_sql: &str = if source_has_user_id {
        "SELECT id, user_id, embedding_vec_1024 FROM memories \
         WHERE embedding_vec_1024 IS NOT NULL AND user_id = ?1"
    } else {
        "SELECT id, embedding_vec_1024 FROM memories \
         WHERE embedding_vec_1024 IS NOT NULL"
    };

    let mut src_stmt = source.conn.prepare(select_sql)?;

    let mut memory_ids: Vec<i64> = Vec::new();
    let mut user_ids: Vec<i64> = Vec::new();
    let mut vectors: Vec<Vec<Option<f32>>> = Vec::new();

    if source_has_user_id {
        let mut rows = src_stmt.query(rusqlite::params![filter_user_id])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let _source_uid: i64 = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;

            if blob.len() == 4096 {
                let vec: Vec<Option<f32>> = blob
                    .chunks_exact(4)
                    .map(|c| Some(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                    .collect();
                memory_ids.push(id);
                user_ids.push(filter_user_id);
                vectors.push(vec);
            } else {
                warn!(
                    "skipping memory {}: embedding blob is {} bytes, expected 4096",
                    id,
                    blob.len()
                );
            }
        }
    } else {
        let mut rows = src_stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;

            if blob.len() == 4096 {
                let vec: Vec<Option<f32>> = blob
                    .chunks_exact(4)
                    .map(|c| Some(f32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                    .collect();
                memory_ids.push(id);
                user_ids.push(filter_user_id);
                vectors.push(vec);
            } else {
                warn!(
                    "skipping memory {}: embedding blob is {} bytes, expected 4096",
                    id,
                    blob.len()
                );
            }
        }
    }

    info!("Extracted {} memory vectors", vectors.len());

    if vectors.is_empty() {
        info!("No vectors to write -- skipping LanceDB write");
        return Ok(());
    }

    let memory_id_array = Int64Array::from(memory_ids);
    let user_id_array = Int64Array::from(user_ids);
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        vectors.into_iter().map(Some),
        DIMENSIONS as i32,
    );

    let schema = vector_schema();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(memory_id_array),
            Arc::new(user_id_array),
            Arc::new(vector_array),
        ],
    )?;

    // Drop and recreate table so re-runs are idempotent.
    let table_names = lance.db.table_names().execute().await?;
    if table_names.iter().any(|n| n == TABLE_NAME) {
        lance.db.drop_table(TABLE_NAME, &[]).await?;
    }

    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let batches: Box<dyn RecordBatchReader + Send> = Box::new(batches);
    let table = lance
        .db
        .create_table(TABLE_NAME, batches)
        .execute()
        .await?;

    info!("Creating vector index...");
    if let Err(e) = table.create_index(&["vector"], Index::Auto).execute().await {
        info!("Vector index creation note (non-fatal): {}", e);
    }

    info!("Vector extraction complete");
    Ok(())
}
