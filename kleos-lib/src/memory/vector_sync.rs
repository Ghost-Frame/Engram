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
use tracing::warn;

/// Deserialize a BLOB (IEEE 754 LE f32 bytes) back into a Vec<f32>.
fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

#[tracing::instrument(skip(db))]
pub async fn build_lance_index_from_existing(db: &Database) -> Result<usize> {
    let Some(index) = db.vector_index.as_ref() else {
        return Ok(0);
    };

    let rows: Vec<(i64, i64, Vec<u8>)> = db
        .read(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, user_id, embedding_vec_1024
                     FROM memories
                     WHERE embedding_vec_1024 IS NOT NULL
                       AND is_forgotten = 0
                       AND is_latest = 1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
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
    for (memory_id, user_id, emb_blob) in rows {
        let embedding = blob_to_embedding(&emb_blob);
        index.insert(memory_id, user_id, &embedding).await?;
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
#[tracing::instrument(skip(db))]
pub async fn replay_vector_sync_pending(
    db: &Database,
    limit: usize,
) -> Result<VectorSyncReplayReport> {
    let mut report = VectorSyncReplayReport::default();
    let Some(index) = db.vector_index.as_ref() else {
        return Ok(report);
    };

    let pending: Vec<(i64, i64, i64, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, memory_id, user_id, op FROM vector_sync_pending \
                     ORDER BY id ASC LIMIT ?1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map(params![limit as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    for (ledger_id, memory_id, user_id, op) in pending {
        report.processed += 1;
        let outcome: std::result::Result<(), String> = match op.as_str() {
            "delete" => index.delete(memory_id).await.map_err(|e| e.to_string()),
            "insert" => {
                let emb_row: Option<Option<Vec<u8>>> = db
                    .read(move |conn| {
                        let result = conn.query_row(
                            "SELECT embedding_vec_1024 \
                             FROM memories WHERE id = ?1 AND user_id = ?2",
                            rusqlite::params![memory_id, user_id],
                            |row| row.get::<_, Option<Vec<u8>>>(0),
                        );
                        match result {
                            Ok(v) => Ok(Some(v)),
                            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                            Err(e) => Err(rusqlite_to_eng_error(e)),
                        }
                    })
                    .await?;
                match emb_row {
                    Some(Some(blob)) => {
                        let embedding = blob_to_embedding(&blob);
                        index
                            .insert(memory_id, user_id, &embedding)
                            .await
                            .map_err(|e| e.to_string())
                    }
                    _ => {
                        report.skipped += 1;
                        Ok(())
                    }
                }
            }
            other => {
                report.skipped += 1;
                warn!("replay skipped unknown vector_sync op '{}'", other);
                Ok(())
            }
        };

        match outcome {
            Ok(()) => {
                db.write(move |conn| {
                    conn.execute(
                        "DELETE FROM vector_sync_pending WHERE id = ?1",
                        params![ledger_id],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                    Ok(())
                })
                .await?;
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

    Ok(report)
}

/// Returns the distinct user_ids that have rows in `vector_sync_pending`.
/// Used by the background task for per-user round-robin scheduling (MT-F17).
#[tracing::instrument(skip(db))]
pub async fn vector_sync_pending_users(db: &Database) -> Result<Vec<i64>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare("SELECT DISTINCT user_id FROM vector_sync_pending ORDER BY user_id ASC")
            .map_err(rusqlite_to_eng_error)?;
        let users: Vec<i64> = stmt
            .query_map([], |row| row.get(0))
            .map_err(rusqlite_to_eng_error)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(users)
    })
    .await
}

/// Same as `replay_vector_sync_pending` but processes only entries belonging
/// to a single user. Called by the per-user round-robin background task (MT-F17).
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

    let pending: Vec<(i64, i64, i64, String)> = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, memory_id, user_id, op FROM vector_sync_pending \
                     WHERE user_id = ?1 ORDER BY id ASC LIMIT ?2",
                )
                .map_err(rusqlite_to_eng_error)?;
            let rows: Vec<_> = stmt
                .query_map(params![user_id, limit as i64], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(rusqlite_to_eng_error)?
                .filter_map(|r| r.ok())
                .collect();
            Ok(rows)
        })
        .await?;

    for (ledger_id, memory_id, uid, op) in pending {
        report.processed += 1;
        let outcome: std::result::Result<(), String> = match op.as_str() {
            "delete" => index.delete(memory_id).await.map_err(|e| e.to_string()),
            "insert" => {
                let emb_row: Option<Option<Vec<u8>>> = db
                    .read(move |conn| {
                        let result = conn.query_row(
                            "SELECT embedding_vec_1024 \
                             FROM memories WHERE id = ?1 AND user_id = ?2",
                            rusqlite::params![memory_id, uid],
                            |row| row.get::<_, Option<Vec<u8>>>(0),
                        );
                        match result {
                            Ok(v) => Ok(Some(v)),
                            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                            Err(e) => Err(rusqlite_to_eng_error(e)),
                        }
                    })
                    .await?;
                match emb_row {
                    Some(Some(blob)) => {
                        let embedding = blob_to_embedding(&blob);
                        index
                            .insert(memory_id, uid, &embedding)
                            .await
                            .map_err(|e| e.to_string())
                    }
                    _ => {
                        report.skipped += 1;
                        Ok(())
                    }
                }
            }
            other => {
                report.skipped += 1;
                warn!("replay skipped unknown vector_sync op '{}'", other);
                Ok(())
            }
        };

        match outcome {
            Ok(()) => {
                db.write(move |conn| {
                    conn.execute(
                        "DELETE FROM vector_sync_pending WHERE id = ?1",
                        params![ledger_id],
                    )
                    .map_err(rusqlite_to_eng_error)?;
                    Ok(())
                })
                .await?;
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

    Ok(report)
}
