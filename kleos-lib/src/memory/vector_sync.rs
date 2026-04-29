//! Vector-sync subsystem: backfill the LanceDB index from existing rows and
//! drain the `vector_sync_pending` ledger left behind by failed writes.
//!
//! Extracted from [`super`] to keep `memory/mod.rs` focused on CRUD. All
//! public items here are re-exported from `memory/mod.rs`, so existing call
//! sites (`kleos_lib::memory::replay_vector_sync_pending`, etc.) continue
//! to resolve unchanged.

use super::rusqlite_to_eng_error;
use super::types::VectorSyncReplayReport;
use crate::db::Database;
use crate::Result;
use rusqlite::params;
use std::collections::HashMap;
use tracing::warn;

/// Batch-fetch embedding blobs for a set of memory IDs.
/// Returns a HashMap keyed by memory_id. Rows whose embedding column is
/// NULL (or that are missing entirely) are simply absent from the map, so
/// callers can treat lookup misses as "skip".
async fn fetch_embeddings_batch(
    db: &Database,
    memory_ids: &[i64],
) -> Result<HashMap<i64, Vec<u8>>> {
    if memory_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let owned: Vec<i64> = memory_ids.to_vec();
    db.read(move |conn| {
        let mut sql = String::from(
            "SELECT id, embedding_vec_1024 FROM memories \
             WHERE embedding_vec_1024 IS NOT NULL AND id IN (",
        );
        for (i, _) in owned.iter().enumerate() {
            if i > 0 {
                sql.push(',');
            }
            sql.push('?');
        }
        sql.push(')');

        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(owned.len());
        for mid in &owned {
            params.push(Box::new(*mid));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;
        let mut map: HashMap<i64, Vec<u8>> = HashMap::with_capacity(owned.len());
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let id: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
            let blob: Vec<u8> = row.get(1).map_err(rusqlite_to_eng_error)?;
            map.insert(id, blob);
        }
        Ok(map)
    })
    .await
}

/// Batch-delete ledger rows by id in a single write.
async fn delete_pending_batch(db: &Database, ledger_ids: Vec<i64>) -> Result<()> {
    if ledger_ids.is_empty() {
        return Ok(());
    }
    db.write(move |conn| {
        let placeholders = ledger_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM vector_sync_pending WHERE id IN ({placeholders})");
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let params: Vec<Box<dyn rusqlite::types::ToSql>> =
            ledger_ids.iter().map(|id| Box::new(*id) as _).collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        stmt.execute(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Deserialize a BLOB (IEEE 754 LE f32 bytes) back into a Vec<f32>.
fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Rebuild the LanceDB vector index from all existing memory embeddings.
/// `_owner_user_id` is retained for call-site compatibility (Stage 18 audit
/// will remove it once all callers are updated).
#[tracing::instrument(skip(db))]
pub async fn build_lance_index_from_existing(db: &Database, _owner_user_id: i64) -> Result<usize> {
    let Some(index) = db.vector_index.as_ref() else {
        return Ok(0);
    };

    let rows: Vec<(i64, Vec<u8>)> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, embedding_vec_1024
                     FROM memories
                     WHERE embedding_vec_1024 IS NOT NULL
                       AND is_forgotten = 0
                       AND is_latest = 1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
                })
                .map_err(rusqlite_to_eng_error)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    let mut count = 0usize;
    for (memory_id, emb_blob) in rows {
        let embedding = blob_to_embedding(&emb_blob);
        index.insert(memory_id, &embedding).await?;
        count += 1;
        #[allow(clippy::manual_is_multiple_of)]
        if count % 1000 == 0 {
            tracing::info!(count, "rebuilt LanceDB vector index rows");
        }
    }

    Ok(count)
}

/// Drain the vector_sync_pending ledger. For each row, retry the failed
/// LanceDB op and remove the row on success. Rows whose underlying memory
/// no longer has an embedding (or has been hard-deleted) are considered
/// skipped and also removed.
///
/// Phase 5.21: user_id is removed from the LanceDB schema. insert() no longer
/// takes a user_id argument; this function passes none.
#[tracing::instrument(skip(db))]
pub async fn replay_vector_sync_pending(
    db: &Database,
    limit: usize,
) -> Result<VectorSyncReplayReport> {
    let mut report = VectorSyncReplayReport::default();
    let Some(index) = db.vector_index.as_ref() else {
        return Ok(report);
    };

    // Tuple: (ledger_id, memory_id, op)
    let pending: Vec<(i64, i64, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, memory_id, op FROM vector_sync_pending \
                     ORDER BY id ASC LIMIT ?1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map(params![limit as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    process_pending_batch(db, index.as_ref(), pending, &mut report).await?;
    Ok(report)
}

/// Process a batch of pending vector-sync rows with batched DB reads and
/// a single batched DELETE for succeeded rows.  Failed rows still take an
/// individual UPDATE because we stamp per-row error text.
async fn process_pending_batch(
    db: &Database,
    index: &dyn crate::vector::VectorIndex,
    pending: Vec<(i64, i64, String)>,
    report: &mut VectorSyncReplayReport,
) -> Result<()> {
    // 1. One SQL read for every `insert` op we are about to retry.
    let insert_ids: Vec<i64> = pending
        .iter()
        .filter(|(_, _, op)| op == "insert")
        .map(|(_, mid, _)| *mid)
        .collect();
    let embeddings = fetch_embeddings_batch(db, &insert_ids).await?;

    // 2. Execute LanceDB ops sequentially (the trait is single-row) and
    //    collect ledger_ids by outcome so we can batch-delete successes.
    let mut succeeded_ids: Vec<i64> = Vec::with_capacity(pending.len());
    for (ledger_id, memory_id, op) in pending {
        report.processed += 1;
        let outcome: std::result::Result<(), String> = match op.as_str() {
            "delete" => index.delete(memory_id).await.map_err(|e| e.to_string()),
            "insert" => match embeddings.get(&memory_id) {
                Some(blob) => {
                    let embedding = blob_to_embedding(blob);
                    index
                        .insert(memory_id, &embedding)
                        .await
                        .map_err(|e| e.to_string())
                }
                None => {
                    report.skipped += 1;
                    Ok(())
                }
            },
            other => {
                report.skipped += 1;
                warn!("replay skipped unknown vector_sync op '{}'", other);
                Ok(())
            }
        };

        match outcome {
            Ok(()) => {
                succeeded_ids.push(ledger_id);
                report.succeeded += 1;
            }
            Err(e) => {
                report.failed += 1;
                warn!("replay failed for memory {} op {}: {}", memory_id, op, e);
                let e_clone = e.clone();
                db.write(move |conn| {
                    conn.execute(
                        "UPDATE vector_sync_pending \
                         SET error = ?1, attempts = attempts + 1, \
                             last_attempt_at = datetime('now') \
                         WHERE id = ?2",
                        params![e_clone, ledger_id],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                    Ok(())
                })
                .await?;
            }
        }
    }

    // 3. One SQL write for the happy path.
    delete_pending_batch(db, succeeded_ids).await?;
    Ok(())
}

/// Returns a Vec of user_ids that have pending entries in `vector_sync_pending`.
/// Used by the background task for per-user round-robin scheduling (MT-F17).
///
/// Phase 5.1: user_id was dropped from vector_sync_pending. The table is now
/// single-tenant (one DB = one owner). We return a single synthetic entry [0]
/// when rows exist so the background task's round-robin loop still fires. The
/// actual user_id is applied at index.insert time via replay_vector_sync_pending_for_user.
#[tracing::instrument(skip(db))]
pub async fn vector_sync_pending_users(db: &Database) -> Result<Vec<i64>> {
    let count: i64 = db
        .read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM vector_sync_pending", [], |row| {
                row.get(0)
            })
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    // Return a single synthetic entry so the background round-robin fires.
    if count > 0 {
        Ok(vec![0])
    } else {
        Ok(vec![])
    }
}

/// Same as `replay_vector_sync_pending` but accepts a `_user_id` retained for
/// call-site compatibility (Stage 18 audit will remove it). Phase 5.21 removed
/// user_id from the LanceDB schema, so the parameter is no longer forwarded to
/// index.insert(). Called by the per-user round-robin background task (MT-F17).
#[tracing::instrument(skip(db))]
pub async fn replay_vector_sync_pending_for_user(
    db: &Database,
    _user_id: i64,
    limit: usize,
) -> Result<VectorSyncReplayReport> {
    let mut report = VectorSyncReplayReport::default();
    let Some(index) = db.vector_index.as_ref() else {
        return Ok(report);
    };

    // Tuple: (ledger_id, memory_id, op)
    let pending: Vec<(i64, i64, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, memory_id, op FROM vector_sync_pending \
                     ORDER BY id ASC LIMIT ?1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map(params![limit as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    process_pending_batch(db, index.as_ref(), pending, &mut report).await?;
    Ok(report)
}

/// Result of a chunk-and-embedding backfill pass.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct BackfillReport {
    pub scanned: usize,
    pub primary_embeddings_filled: usize,
    pub chunk_rows_written: usize,
    pub failures: usize,
}

/// Walk every active memory that is missing either a primary embedding
/// (`embedding_vec_1024 IS NULL`) or any rows in `memory_chunks`, and use
/// `embedder` to populate them. This is the path called by the admin
/// backfill route on production deploys (after migration 51 lands and
/// existing memories haven't been chunked yet) and by harness-seed (which
/// inserts memory rows via raw SQL and never gets to `memory::store`).
///
/// The function is best-effort and rate-limited (one memory at a time
/// with a small sleep between iterations) because the ONNX session is a
/// Mutex and concurrency does not help. Failures are counted; the loop
/// continues so a single bad row does not block the rest.
#[tracing::instrument(skip(db, embedder))]
pub async fn backfill_missing_embeddings(
    db: &Database,
    embedder: &dyn crate::embeddings::EmbeddingProvider,
) -> Result<BackfillReport> {
    let chunk_max_chars = db.embedding_chunk_max_chars;
    let chunk_overlap = db.embedding_chunk_overlap;
    let chunk_max_chunks = db.embedding_chunk_max_chunks;

    let candidates: Vec<(i64, String, bool, bool)> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT m.id, m.content, \
                            CASE WHEN m.embedding_vec_1024 IS NULL THEN 1 ELSE 0 END AS need_primary, \
                            CASE WHEN NOT EXISTS (SELECT 1 FROM memory_chunks mc WHERE mc.memory_id = m.id) THEN 1 ELSE 0 END AS need_chunks \
                     FROM memories m \
                     WHERE m.is_forgotten = 0 AND m.is_latest = 1 AND TRIM(m.content) != '' \
                       AND (m.embedding_vec_1024 IS NULL \
                            OR NOT EXISTS (SELECT 1 FROM memory_chunks mc WHERE mc.memory_id = m.id))",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)? != 0,
                        row.get::<_, i64>(3)? != 0,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    let mut report = BackfillReport {
        scanned: candidates.len(),
        ..Default::default()
    };

    for (memory_id, content, need_primary, need_chunks) in candidates {
        if need_primary {
            match embedder.embed(&content).await {
                Ok(emb) => {
                    if let Err(e) = persist_primary_embedding(db, memory_id, &emb).await {
                        warn!("primary embedding persist failed for {}: {}", memory_id, e);
                        report.failures += 1;
                    } else {
                        report.primary_embeddings_filled += 1;
                    }
                }
                Err(e) => {
                    warn!("embed failed for {}: {}", memory_id, e);
                    report.failures += 1;
                }
            }
        }

        if need_chunks {
            match crate::embeddings::chunking::chunk_and_embed(
                embedder,
                &content,
                chunk_max_chars,
                chunk_overlap,
                chunk_max_chunks,
            )
            .await
            {
                Ok(pairs) if !pairs.is_empty() => {
                    let n = pairs.len();
                    super::write_chunks(db, memory_id, &pairs).await;
                    report.chunk_rows_written += n;
                }
                Ok(_) => {}
                Err(e) => {
                    warn!("chunk_and_embed failed for {}: {}", memory_id, e);
                    report.failures += 1;
                }
            }
        }

        // Light rate-limit: ONNX session is a single-threaded mutex, so
        // queuing aggressively just adds contention. 50ms keeps a backfill
        // of ~10k memories under 10 minutes.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    Ok(report)
}

async fn persist_primary_embedding(db: &Database, memory_id: i64, emb: &[f32]) -> Result<()> {
    let blob: Vec<u8> = emb.iter().flat_map(|f| f.to_le_bytes()).collect();
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET embedding_vec_1024 = ?1 WHERE id = ?2",
            params![blob, memory_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    if let Some(index) = db.vector_index.as_ref() {
        index.insert(memory_id, emb).await?;
    }
    Ok(())
}

#[tracing::instrument(skip(db))]
pub async fn build_lance_chunk_index_from_existing(db: &Database) -> Result<usize> {
    let Some(index) = db.chunk_vector_index.as_ref() else {
        return Ok(0);
    };

    let rows: Vec<(i64, usize, Vec<u8>)> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT mc.memory_id, mc.chunk_idx, mc.embedding_vec_1024
                     FROM memory_chunks mc
                     JOIN memories m ON m.id = mc.memory_id
                     WHERE mc.embedding_vec_1024 IS NOT NULL
                       AND m.is_forgotten = 0
                       AND m.is_latest = 1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, usize>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    let mut count = 0usize;
    for (memory_id, chunk_idx, emb_blob) in rows {
        let embedding = blob_to_embedding(&emb_blob);
        let key = super::chunk_lance_key(memory_id, chunk_idx);
        index.insert(key, &embedding).await?;
        count += 1;
        #[allow(clippy::manual_is_multiple_of)]
        if count % 1000 == 0 {
            tracing::info!(count, "rebuilt LanceDB chunk vector index rows");
        }
    }

    Ok(count)
}
