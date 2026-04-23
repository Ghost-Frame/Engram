use anyhow::{anyhow, Result};
use arrow_array::cast::AsArray;
use arrow_array::types::Float32Type;
use arrow_array::{Array, FixedSizeListArray, Int64Array, RecordBatch, RecordBatchIterator, RecordBatchReader};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
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

/// Open an existing LanceDB directory for read-only access (source side).
/// Points at the parent directory, not the `.lance` table itself -- lancedb
/// treats the parent as the "database" and the `.lance` directory as a table
/// inside it.
async fn open_source_lance_parent(path: &Path) -> Result<lancedb::Connection> {
    if !path.exists() {
        return Err(anyhow!(
            "--source-lance path does not exist: {}",
            path.display()
        ));
    }
    // If the caller passed the .lance table directly, point lancedb at its
    // parent directory. Both shapes are common on disk.
    let connect_dir = if path.extension().and_then(|s| s.to_str()) == Some("lance") {
        path.parent()
            .ok_or_else(|| anyhow!("source-lance has no parent: {}", path.display()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    };
    let conn = lancedb::connect(connect_dir.to_string_lossy().as_ref())
        .execute()
        .await?;
    Ok(conn)
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

/// Stats from one vector extraction pass. Used by validate::run to catch
/// the silent-drop case where the source had eligible embeddings but
/// everything got filtered out by the blob-size check.
#[derive(Debug, Default, Clone, Copy)]
pub struct VectorStats {
    pub source_eligible: usize,
    pub inserted: usize,
}

/// Extract embeddings from source memories and write them into LanceDB.
pub async fn extract_and_insert(
    source: &SourceDb,
    lance: &LanceDb,
    filter_user_id: i64,
) -> Result<VectorStats> {
    info!("Extracting memory vectors...");

    let source_cols = source::get_columns(source, "memories")?;
    let source_has_user_id = source_cols.iter().any(|c| c == "user_id");
    let source_has_vec_col = source_cols.iter().any(|c| c == "embedding_vec_1024");

    if !source_has_vec_col {
        info!("source memories table has no embedding_vec_1024 column -- skipping vector extraction");
        return Ok(VectorStats::default());
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
    let mut source_eligible = 0usize;

    if source_has_user_id {
        let mut rows = src_stmt.query(rusqlite::params![filter_user_id])?;
        while let Some(row) = rows.next()? {
            source_eligible += 1;
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
            source_eligible += 1;
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

    let inserted = vectors.len();
    info!("Extracted {} / {} memory vectors", inserted, source_eligible);

    if vectors.is_empty() {
        info!("No vectors to write -- skipping LanceDB write");
        return Ok(VectorStats {
            source_eligible,
            inserted,
        });
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
    Ok(VectorStats {
        source_eligible,
        inserted,
    })
}

/// Copy vectors from a source LanceDB directory into the target LanceDB,
/// filtering by user_id. The source is expected to hold the shared table
/// name "memory_vectors" with schema (memory_id i64, user_id i64, vector
/// FixedSizeList<Float32, 1024>) -- the same schema the monolith writes
/// via kleos_lib::vector::lance::vector_schema.
///
/// Used when the monolith keeps embeddings in LanceDB rather than in the
/// memories.embedding_vec_1024 SQL column.
pub async fn extract_from_source_lance(
    source_lance: &Path,
    target: &LanceDb,
    filter_user_id: i64,
) -> Result<VectorStats> {
    info!("Extracting vectors from source LanceDB at {:?}", source_lance);

    let src_conn = open_source_lance_parent(source_lance).await?;
    let table_names = src_conn.table_names().execute().await?;
    if !table_names.iter().any(|n| n == TABLE_NAME) {
        return Err(anyhow!(
            "source LanceDB has no '{}' table; found: {:?}",
            TABLE_NAME,
            table_names
        ));
    }
    let src_table = src_conn.open_table(TABLE_NAME).execute().await?;

    // Filter on the fly so we only hold one tenant's rows in memory.
    let filter_expr = format!("user_id = {}", filter_user_id);
    let mut stream = src_table
        .query()
        .only_if(filter_expr.clone())
        .execute()
        .await?;

    let mut memory_ids: Vec<i64> = Vec::new();
    let mut user_ids: Vec<i64> = Vec::new();
    let mut vectors: Vec<Vec<Option<f32>>> = Vec::new();
    let mut source_eligible = 0usize;

    while let Some(batch) = stream.try_next().await? {
        source_eligible += batch.num_rows();
        let mid_col = batch
            .column_by_name("memory_id")
            .ok_or_else(|| anyhow!("source lance missing memory_id column"))?
            .as_primitive::<arrow_array::types::Int64Type>()
            .clone();
        let uid_col = batch
            .column_by_name("user_id")
            .ok_or_else(|| anyhow!("source lance missing user_id column"))?
            .as_primitive::<arrow_array::types::Int64Type>()
            .clone();
        let vec_col = batch
            .column_by_name("vector")
            .ok_or_else(|| anyhow!("source lance missing vector column"))?
            .as_fixed_size_list()
            .clone();

        for i in 0..batch.num_rows() {
            let mid = mid_col.value(i);
            let _uid = uid_col.value(i);
            let values = vec_col.value(i);
            let floats = values.as_primitive::<Float32Type>();
            if floats.len() != DIMENSIONS {
                warn!(
                    "skipping memory {}: vector length {} != {}",
                    mid,
                    floats.len(),
                    DIMENSIONS
                );
                continue;
            }
            let v: Vec<Option<f32>> = (0..DIMENSIONS)
                .map(|j| {
                    if floats.is_null(j) {
                        None
                    } else {
                        Some(floats.value(j))
                    }
                })
                .collect();
            memory_ids.push(mid);
            user_ids.push(filter_user_id);
            vectors.push(v);
        }
    }

    let inserted = vectors.len();
    info!(
        "Extracted {} / {} memory vectors from source LanceDB",
        inserted, source_eligible
    );

    if vectors.is_empty() {
        info!("No vectors matched user_id filter -- skipping LanceDB write");
        return Ok(VectorStats {
            source_eligible,
            inserted,
        });
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

    // Drop and recreate so re-runs are idempotent.
    let dst_tables = target.db.table_names().execute().await?;
    if dst_tables.iter().any(|n| n == TABLE_NAME) {
        target.db.drop_table(TABLE_NAME, &[]).await?;
    }
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let batches: Box<dyn RecordBatchReader + Send> = Box::new(batches);
    let dst_table = target
        .db
        .create_table(TABLE_NAME, batches)
        .execute()
        .await?;

    info!("Creating vector index on target...");
    if let Err(e) = dst_table
        .create_index(&["vector"], Index::Auto)
        .execute()
        .await
    {
        info!("Vector index creation note (non-fatal): {}", e);
    }

    Ok(VectorStats {
        source_eligible,
        inserted,
    })
}

/// Count rows in source LanceDB matching a user_id filter. Used by the
/// dry-run report so operators can see vector counts before committing.
pub async fn dry_run_source_lance_count(source_lance: &Path, filter_user_id: i64) -> Result<usize> {
    let src_conn = open_source_lance_parent(source_lance).await?;
    let table_names = src_conn.table_names().execute().await?;
    if !table_names.iter().any(|n| n == TABLE_NAME) {
        return Ok(0);
    }
    let src_table = src_conn.open_table(TABLE_NAME).execute().await?;
    let filter_expr = format!("user_id = {}", filter_user_id);
    let mut stream = src_table.query().only_if(filter_expr).execute().await?;
    let mut count = 0usize;
    while let Some(batch) = stream.try_next().await? {
        count += batch.num_rows();
    }
    Ok(count)
}
