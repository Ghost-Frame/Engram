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
/// `owner_user_id` is used as the user_id field in the LanceDB record (the
/// memories table no longer stores user_id, so the caller supplies it).
#[tracing::instrument(skip(db))]
pub async fn build_lance_index_from_existing(db: &Database, owner_user_id: i64) -> Result<usize> {
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
        index.insert(memory_id, owner_user_id, &embedding).await?;
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
/// Drain the vector_sync_pending ledger. For each row, retry the failed
/// LanceDB op and remove the row on success. Rows whose underlying memory
/// no longer has an embedding (or has been hard-deleted) are considered
/// skipped and also removed.
///
/// `owner_user_id` is passed to LanceDB insert calls because user_id is no
/// longer stored in vector_sync_pending (Phase 5.1).
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

    // Phase 5.1 transitional: owner_user_id is no longer stored in
    // vector_sync_pending. Callers that know the tenant owner should use
    // replay_vector_sync_pending_for_user. This generic path stamps 0 as a
    // sentinel because LanceDB still carries a user_id field until Phase 5.21;
    // searches that filter by user_id will NOT match these rows. The table is
    // empty on production shards today, so hitting this warn! is a real signal.
    if !pending.is_empty() {
        warn!(
            pending_rows = pending.len(),
            "replay_vector_sync_pending using owner_user_id=0 sentinel; LanceDB rows may be invisible to per-user filters until Phase 5.21"
        );
    }
    process_pending_batch(db, index.as_ref(), pending, 0, &mut report).await?;
    Ok(report)
}

/// Process a batch of pending vector-sync rows with batched DB reads and
/// a single batched DELETE for succeeded rows.  Failed rows still take an
/// individual UPDATE because we stamp per-row error text.
///
/// `owner_user_id` is passed to LanceDB insert calls because user_id is no
/// longer stored in vector_sync_pending (Phase 5.1). The LanceDB schema
/// still carries the field until Phase 5.21.
async fn process_pending_batch(
    db: &Database,
    index: &dyn crate::vector::VectorIndex,
    pending: Vec<(i64, i64, String)>,
    owner_user_id: i64,
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
                        .insert(memory_id, owner_user_id, &embedding)
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

/// Same as `replay_vector_sync_pending` but accepts a `user_id` that is used
/// as `owner_user_id` for LanceDB insert calls. Phase 5.1 removed user_id from
/// the vector_sync_pending table; the WHERE clause is dropped accordingly.
/// Called by the per-user round-robin background task (MT-F17).
#[tracing::instrument(skip(db))]
pub async fn replay_vector_sync_pending_for_user(
    db: &Database,
    user_id: i64,
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

    process_pending_batch(db, index.as_ref(), pending, user_id, &mut report).await?;
    Ok(report)
}
