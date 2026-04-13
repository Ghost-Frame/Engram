use anyhow::Result;
use arrow_array::types::Float32Type;
use arrow_array::{FixedSizeListArray, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader};
use arrow_schema::{DataType, Field, Schema};
use lancedb::index::Index;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

use crate::source::SourceDb;

const TABLE_NAME: &str = "memory_vectors";
const DIMENSIONS: usize = 1024;

pub struct LanceDb {
    pub db: lancedb::Connection,
}

/// Open or create LanceDB at target/lance/
pub async fn open_lance(target_dir: &Path) -> Result<LanceDb> {
    let lance_path = target_dir.join("lance");
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

/// Extract embeddings from source and insert into LanceDB
pub async fn extract_and_insert(source: &SourceDb, lance: &LanceDb) -> Result<()> {
    info!("Extracting memory vectors...");

    // Query memories with embeddings
    let mut stmt = source
        .conn
        .prepare(
            "SELECT id, user_id, embedding_vec_1024 FROM memories WHERE embedding_vec_1024 IS NOT NULL",
        )
        .await?;

    let mut rows = stmt.query(()).await?;

    let mut memory_ids: Vec<i64> = Vec::new();
    let mut user_ids: Vec<i64> = Vec::new();
    let mut vectors: Vec<Vec<Option<f32>>> = Vec::new();

    while let Some(row) = rows.next().await? {
        let memory_id: i64 = row.get(0)?;
        let user_id: i64 = row.get(1)?;
        let embedding_blob: Vec<u8> = row.get(2)?;

        // Decode FLOAT32(1024) blob: 1024 floats * 4 bytes each = 4096 bytes
        if embedding_blob.len() == 4096 {
            let vector: Vec<Option<f32>> = embedding_blob
                .chunks_exact(4)
                .map(|chunk| Some(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])))
                .collect();

            memory_ids.push(memory_id);
            user_ids.push(user_id);
            vectors.push(vector);
        }
    }

    info!("Extracted {} memory vectors", vectors.len());

    if vectors.is_empty() {
        return Ok(());
    }

    // Build arrow arrays
    let memory_id_array = Int64Array::from(memory_ids);
    let user_id_array = Int64Array::from(user_ids);

    // Build FixedSizeListArray from vectors (wrap in Some for the API)
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

    // Check if table exists
    let table_names = lance.db.table_names().execute().await?;

    let table = if table_names.iter().any(|name| name == TABLE_NAME) {
        // Drop and recreate
        lance.db.drop_table(TABLE_NAME, &[]).await?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let batches: Box<dyn RecordBatchReader + Send> = Box::new(batches);
        lance
            .db
            .create_table(TABLE_NAME, batches)
            .execute()
            .await?
    } else {
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let batches: Box<dyn RecordBatchReader + Send> = Box::new(batches);
        lance
            .db
            .create_table(TABLE_NAME, batches)
            .execute()
            .await?
    };

    // Create vector index
    info!("Creating vector index...");
    if let Err(e) = table.create_index(&["vector"], Index::Auto).execute().await {
        info!("Vector index creation note: {}", e);
    }

    info!("Vector extraction complete");
    Ok(())
}
