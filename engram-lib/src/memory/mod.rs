pub mod fts;
pub mod scoring;
pub mod search;
pub mod simhash;
pub mod types;
pub mod vector;

use crate::db::Database;
use crate::personality;
use crate::EngError;
use crate::Result;
use rusqlite::params;
use std::collections::{BTreeMap, HashMap, HashSet};
use tracing::warn;
use types::{
    CategoryCount, LinkedMemory, ListOptions, Memory, StoreRequest, StoreResult, TagCount,
    UpdateRequest, UserProfile, UserStats, VersionChainEntry,
};

// -- Constants ---

const MAX_CONTENT_SIZE: usize = 102400; // 100KB

// -- Helpers ---

fn normalize_tags(tags: &Option<Vec<String>>) -> Option<String> {
    tags.as_ref().and_then(|t| {
        let normalized: Vec<String> = t
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        if normalized.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&normalized).unwrap())
        }
    })
}

fn parse_tags_json(tags: &Option<String>) -> Vec<String> {
    tags.as_ref()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default()
}

fn clamp_importance(value: i32) -> i32 {
    value.clamp(1, 10)
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Record a failed LanceDB write into the vector_sync_pending table so a
/// sweeper (or the admin replay endpoint) can retry it. Intentionally
/// best-effort: if the sync-pending insert itself fails, log and move on.
async fn record_vector_sync_failure(
    db: &Database,
    memory_id: i64,
    user_id: i64,
    op: &str,
    err: &str,
) {
    let op_owned = op.to_string();
    let err_owned = err.to_string();
    let op_for_log = op_owned.clone();
    let result = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO vector_sync_pending (memory_id, user_id, op, error) \
                 VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![memory_id, user_id, op_owned, err_owned],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await;
    if let Err(e) = result {
        warn!(
            "failed to record vector_sync_pending for memory {} ({}) : {}",
            memory_id, op_for_log, e
        );
    }
}

/// Serialize an embedding as raw IEEE 754 little-endian bytes for BLOB storage.
/// This is the same wire format that libsql's `vector()` function produces,
/// so existing FLOAT32(1024) columns can be read by either backend.
fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(embedding.len() * 4);
    for &f in embedding {
        buf.extend_from_slice(&f.to_le_bytes());
    }
    buf
}

/// Deserialize a BLOB (IEEE 754 LE f32 bytes) back into a Vec<f32>.
fn blob_to_embedding(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Map a rusqlite Row to a Memory struct.
/// Column order must match the MEMORY_COLUMNS constant below.
/// Order: id, content, category, source, session_id, importance, version,
///   is_latest, parent_memory_id, root_memory_id, source_count, is_static,
///   is_forgotten, is_archived, is_fact, is_decomposed,
///   forget_after, forget_reason, model, recall_hits, recall_misses,
///   adaptive_score, pagerank_score, last_accessed_at, access_count, tags,
///   episode_id, decay_score, confidence, sync_id, status, user_id, space_id,
///   fsrs_stability, fsrs_difficulty, fsrs_storage_strength,
///   fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses,
///   fsrs_last_review_at, valence, arousal, dominant_emotion,
///   created_at, updated_at, is_superseded, is_consolidated
pub(crate) fn row_to_memory(row: &rusqlite::Row<'_>) -> Result<Memory> {
    Ok(Memory {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        content: row.get(1).map_err(rusqlite_to_eng_error)?,
        category: row.get(2).map_err(rusqlite_to_eng_error)?,
        source: row.get(3).map_err(rusqlite_to_eng_error)?,
        session_id: row.get(4).map_err(rusqlite_to_eng_error)?,
        importance: row.get(5).map_err(rusqlite_to_eng_error)?,
        embedding: None,
        version: row.get(6).map_err(rusqlite_to_eng_error)?,
        is_latest: row.get::<_, i32>(7).map_err(rusqlite_to_eng_error)? != 0,
        parent_memory_id: row.get(8).map_err(rusqlite_to_eng_error)?,
        root_memory_id: row.get(9).map_err(rusqlite_to_eng_error)?,
        source_count: row.get(10).map_err(rusqlite_to_eng_error)?,
        is_static: row.get::<_, i32>(11).map_err(rusqlite_to_eng_error)? != 0,
        is_forgotten: row.get::<_, i32>(12).map_err(rusqlite_to_eng_error)? != 0,
        is_archived: row.get::<_, i32>(13).map_err(rusqlite_to_eng_error)? != 0,
        is_fact: row.get::<_, i32>(14).map_err(rusqlite_to_eng_error)? != 0,
        is_decomposed: row.get::<_, i32>(15).map_err(rusqlite_to_eng_error)? != 0,
        forget_after: row.get(16).map_err(rusqlite_to_eng_error)?,
        forget_reason: row.get(17).map_err(rusqlite_to_eng_error)?,
        model: row.get(18).map_err(rusqlite_to_eng_error)?,
        recall_hits: row.get(19).map_err(rusqlite_to_eng_error)?,
        recall_misses: row.get(20).map_err(rusqlite_to_eng_error)?,
        adaptive_score: row.get(21).map_err(rusqlite_to_eng_error)?,
        pagerank_score: row.get(22).map_err(rusqlite_to_eng_error)?,
        last_accessed_at: row.get(23).map_err(rusqlite_to_eng_error)?,
        access_count: row.get(24).map_err(rusqlite_to_eng_error)?,
        tags: row.get(25).map_err(rusqlite_to_eng_error)?,
        episode_id: row.get(26).map_err(rusqlite_to_eng_error)?,
        decay_score: row.get(27).map_err(rusqlite_to_eng_error)?,
        confidence: row.get(28).map_err(rusqlite_to_eng_error)?,
        sync_id: row.get(29).map_err(rusqlite_to_eng_error)?,
        status: row.get(30).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(31).map_err(rusqlite_to_eng_error)?,
        space_id: row.get(32).map_err(rusqlite_to_eng_error)?,
        fsrs_stability: row.get(33).map_err(rusqlite_to_eng_error)?,
        fsrs_difficulty: row.get(34).map_err(rusqlite_to_eng_error)?,
        fsrs_storage_strength: row.get(35).map_err(rusqlite_to_eng_error)?,
        fsrs_retrieval_strength: row.get(36).map_err(rusqlite_to_eng_error)?,
        fsrs_learning_state: row.get(37).map_err(rusqlite_to_eng_error)?,
        fsrs_reps: row.get(38).map_err(rusqlite_to_eng_error)?,
        fsrs_lapses: row.get(39).map_err(rusqlite_to_eng_error)?,
        fsrs_last_review_at: row.get(40).map_err(rusqlite_to_eng_error)?,
        valence: row.get(41).map_err(rusqlite_to_eng_error)?,
        arousal: row.get(42).map_err(rusqlite_to_eng_error)?,
        dominant_emotion: row.get(43).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(44).map_err(rusqlite_to_eng_error)?,
        updated_at: row.get(45).map_err(rusqlite_to_eng_error)?,
        is_superseded: row.get::<_, i32>(46).map_err(rusqlite_to_eng_error)? != 0,
        is_consolidated: row.get::<_, i32>(47).map_err(rusqlite_to_eng_error)? != 0,
    })
}

/// Standard SELECT column list -- matches row_to_memory index order.
pub(crate) const MEMORY_COLUMNS: &str = "id, content, category, source, session_id, importance, \
    version, is_latest, parent_memory_id, root_memory_id, source_count, is_static, \
    is_forgotten, is_archived, is_fact, is_decomposed, \
    forget_after, forget_reason, model, recall_hits, recall_misses, \
    adaptive_score, pagerank_score, last_accessed_at, access_count, tags, \
    episode_id, decay_score, confidence, sync_id, status, user_id, space_id, \
    fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, \
    fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, \
    valence, arousal, dominant_emotion, created_at, updated_at, is_superseded, is_consolidated";

// -- Public CRUD functions ---

pub async fn store(db: &Database, req: StoreRequest) -> Result<StoreResult> {
    // 1. Validate content
    let content = req.content.trim().to_string();
    if content.is_empty() {
        return Err(EngError::InvalidInput(
            "content cannot be empty".to_string(),
        ));
    }
    if content.len() > MAX_CONTENT_SIZE {
        return Err(EngError::InvalidInput(format!(
            "content exceeds maximum size of {} bytes",
            MAX_CONTENT_SIZE
        )));
    }

    // SECURITY: previous code defaulted to user 1 (typically the bootstrap
    // admin). A caller that forgot to set user_id would silently attribute
    // the memory to tenant 1 -- fail closed instead.
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id required".into()))?;

    // SECURITY (MT-F20): enforce tenant memory quota on every write path.
    crate::quota::enforce_memory_quota(db, user_id).await?;

    let importance = clamp_importance(req.importance);

    // 2. Compute simhash of content
    let content_hash = simhash::simhash(&content);

    // 3. Check for duplicates
    let dup_sql = "SELECT id, content FROM memories \
        WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0 \
        ORDER BY id DESC LIMIT 1000";

    let duplicate = db
        .read(move |conn| {
            let mut stmt = conn.prepare(dup_sql).map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![user_id])
                .map_err(rusqlite_to_eng_error)?;
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let existing_id: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
                let existing_content: String = row.get(1).map_err(rusqlite_to_eng_error)?;
                let existing_hash = simhash::simhash(&existing_content);
                if simhash::hamming_distance(content_hash, existing_hash) < 3 {
                    return Ok(Some(existing_id));
                }
            }
            Ok(None)
        })
        .await?;

    if let Some(existing_id) = duplicate {
        return Ok(StoreResult {
            id: existing_id,
            created: false,
            duplicate_of: Some(existing_id),
        });
    }

    let tags_json = normalize_tags(&req.tags);
    let content_for_tx = content.clone();
    let req_for_tx = req.clone();
    let tags_json_for_tx = tags_json.clone();

    let new_id = db
        .transaction(move |tx| {
            store_transactional_rusqlite(
                tx,
                &content_for_tx,
                &req_for_tx,
                user_id,
                importance,
                tags_json_for_tx,
            )
        })
        .await?;

    if let Some(ref emb) = req.embedding {
        if let Some(index) = db.vector_index.as_ref() {
            if let Err(e) = index.insert(new_id, user_id, emb).await {
                warn!("LanceDB vector insert failed for memory {}: {}", new_id, e);
                record_vector_sync_failure(db, new_id, user_id, "insert", &e.to_string()).await;
            }
        }
    }

    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, user_id, 1).await {
        warn!("pagerank dirty mark failed on store: {}", e);
    }

    Ok(StoreResult {
        id: new_id,
        created: true,
        duplicate_of: None,
    })
}

fn store_transactional_rusqlite(
    tx: &rusqlite::Transaction<'_>,
    content: &str,
    req: &StoreRequest,
    user_id: i64,
    importance: i32,
    tags_json: Option<String>,
) -> Result<i64> {
    let (version, root_memory_id) = if let Some(parent_id) = req.parent_memory_id {
        let mut stmt = tx
            .prepare("SELECT version, root_memory_id FROM memories WHERE id = ?1 AND user_id = ?2")
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![parent_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        if let Some(parent_row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let parent_version: i32 = parent_row.get(0).map_err(rusqlite_to_eng_error)?;
            let parent_root: Option<i64> = parent_row.get(1).map_err(rusqlite_to_eng_error)?;
            let root = parent_root.unwrap_or(parent_id);
            (parent_version + 1, Some(root))
        } else {
            return Err(EngError::NotFound(format!(
                "parent memory {} not found or not owned by user",
                parent_id
            )));
        }
    } else {
        (1, None)
    };

    if let Some(parent_id) = req.parent_memory_id {
        tx.execute(
            "UPDATE memories SET is_latest = 0, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![parent_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
    }

    let is_static = req.is_static.unwrap_or(false) as i32;
    tx.execute(
        "INSERT INTO memories (
            content, category, source, session_id, importance,
            version, is_latest, parent_memory_id, root_memory_id,
            is_static, tags, status, user_id, space_id,
            fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state,
            fsrs_reps, fsrs_lapses, model
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, 1, ?7, ?8,
            ?9, ?10, 'approved', ?11, ?12,
            1.0, 1.0, 0,
            0, 0, ?13
        )",
        rusqlite::params![
            content,
            req.category.clone(),
            req.source.clone(),
            req.session_id.clone(),
            importance,
            version,
            req.parent_memory_id,
            root_memory_id,
            is_static,
            tags_json,
            user_id,
            req.space_id,
            Option::<String>::None
        ],
    )
    .map_err(rusqlite_to_eng_error)?;

    let new_id = tx.last_insert_rowid();

    if let Some(ref emb) = req.embedding {
        let emb_blob = embedding_to_blob(emb);
        tx.execute(
            "UPDATE memories SET embedding_vec_1024 = ?1 WHERE id = ?2",
            rusqlite::params![emb_blob, new_id],
        )
        .map_err(rusqlite_to_eng_error)?;
    }

    Ok(new_id)
}

/// Retrieve a memory by ID for content access. Filters out forgotten and archived memories.
pub async fn get(db: &Database, id: i64, user_id: i64) -> Result<Memory> {
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0 AND is_archived = 0",
        MEMORY_COLUMNS
    );
    get_internal(db, id, user_id, &sql, true).await
}

/// Retrieve a memory by ID for ownership/existence checks. Only filters forgotten memories,
/// allowing archived memories to be returned. Use this for permission checks, link targets,
/// and version chain lookups where the memory must exist but doesn't need to be active.
pub async fn get_for_ownership(db: &Database, id: i64, user_id: i64) -> Result<Memory> {
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0",
        MEMORY_COLUMNS
    );
    get_internal(db, id, user_id, &sql, false).await
}

async fn get_internal(
    db: &Database,
    id: i64,
    user_id: i64,
    sql: &str,
    log_access: bool,
) -> Result<Memory> {
    let sql_for_read = sql.to_string();
    let memory = db
        .read(move |conn| {
            let mut stmt = conn.prepare(&sql_for_read).map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![id, user_id])
                .map_err(rusqlite_to_eng_error)?;
            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                row_to_memory(row)
            } else {
                Err(EngError::NotFound(format!("memory {} not found", id)))
            }
        })
        .await?;

    if log_access {
        db.write(move |conn| {
            conn.execute(
                "UPDATE memories SET \
                    access_count = access_count + 1, \
                    last_accessed_at = datetime('now'), \
                    updated_at = datetime('now') \
                 WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await?;
    }

    Ok(memory)
}

pub async fn list(db: &Database, opts: ListOptions) -> Result<Vec<Memory>> {
    // SECURITY (SEC-C3): user_id MUST be set. Without a tenant filter the
    // query returns every user's memories. All HTTP handlers set this, but
    // a missing guard here would silently expose all data if any future
    // internal caller uses ListOptions::default().
    if opts.user_id.is_none() {
        return Err(crate::EngError::InvalidInput(
            "user_id is required for memory listing".into(),
        ));
    }

    // Build WHERE clauses with parameterized values to prevent SQL injection
    let mut conditions = vec!["1=1".to_string()];
    let mut param_values: Vec<rusqlite::types::Value> = Vec::new();
    let mut param_idx = 1;

    if !opts.include_forgotten {
        conditions.push("is_forgotten = 0".to_string());
    }
    if !opts.include_archived {
        conditions.push("is_archived = 0".to_string());
    }
    // Always filter to latest version and hide consolidated sources
    conditions.push("is_latest = 1".to_string());
    conditions.push("is_consolidated = 0".to_string());

    if let Some(ref cat) = opts.category {
        conditions.push(format!("category = ?{}", param_idx));
        param_values.push(rusqlite::types::Value::Text(cat.clone()));
        param_idx += 1;
    }
    if let Some(ref src) = opts.source {
        conditions.push(format!("source = ?{}", param_idx));
        param_values.push(rusqlite::types::Value::Text(src.clone()));
        param_idx += 1;
    }
    if let Some(uid) = opts.user_id {
        conditions.push(format!("user_id = ?{}", param_idx));
        param_values.push(rusqlite::types::Value::Integer(uid));
        param_idx += 1;
    }
    if let Some(sid) = opts.space_id {
        conditions.push(format!("space_id = ?{}", param_idx));
        param_values.push(rusqlite::types::Value::Integer(sid));
        param_idx += 1;
    }

    // Add limit and offset as parameters
    conditions.push("1=1".to_string()); // placeholder for LIMIT/OFFSET which go after WHERE
    let where_clause = conditions.join(" AND ");

    let sql = format!(
        "SELECT {} FROM memories WHERE {} ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
        MEMORY_COLUMNS,
        where_clause,
        param_idx,
        param_idx + 1
    );
    param_values.push(rusqlite::types::Value::Integer(opts.limit as i64));
    param_values.push(rusqlite::types::Value::Integer(opts.offset as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rusqlite_params = rusqlite::params_from_iter(param_values.iter().cloned());
        let mut rows = stmt.query(rusqlite_params).map_err(rusqlite_to_eng_error)?;
        let mut memories = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            memories.push(row_to_memory(row)?);
        }
        Ok(memories)
    })
    .await
}

pub async fn delete(db: &Database, id: i64, user_id: i64) -> Result<()> {
    // Soft delete -- set is_forgotten, record reason
    let affected = db
        .write(move |conn| {
            conn.execute(
                "UPDATE memories SET \
                    is_forgotten = 1, \
                    forget_reason = 'user_deleted', \
                    updated_at = datetime('now') \
                 WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "memory {} not found or already deleted",
            id
        )));
    }
    if let Some(index) = db.vector_index.as_ref() {
        if let Err(e) = index.delete(id).await {
            warn!("LanceDB vector delete failed for memory {}: {}", id, e);
            record_vector_sync_failure(db, id, user_id, "delete", &e.to_string()).await;
        }
    }
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, user_id, 1).await {
        warn!(
            "mark_pagerank_dirty failed after delete for user {}: {}",
            user_id, e
        );
    }
    Ok(())
}

/// List soft-deleted memories for a user (recovery window).
/// Only returns memories deleted by the user (`forget_reason = 'user_deleted'`),
/// not system-initiated forgets (consolidation, contradiction, etc.).
pub async fn list_trashed(db: &Database, user_id: i64, limit: usize) -> Result<Vec<Memory>> {
    let sql = format!(
        "SELECT {} FROM memories \
         WHERE user_id = ?1 AND is_forgotten = 1 AND forget_reason = 'user_deleted' \
         ORDER BY updated_at DESC LIMIT ?2",
        MEMORY_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id, limit as i64])
            .map_err(rusqlite_to_eng_error)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            result.push(row_to_memory(row)?);
        }
        Ok(result)
    })
    .await
}

/// Restore a soft-deleted memory (undo user delete).
/// Returns the restored memory. Fails if the memory is not in a user-deleted state.
pub async fn restore(db: &Database, id: i64, user_id: i64) -> Result<Memory> {
    let affected = db
        .write(move |conn| {
            conn.execute(
                "UPDATE memories SET \
                    is_forgotten = 0, \
                    forget_reason = NULL, \
                    updated_at = datetime('now') \
                 WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 1 AND forget_reason = 'user_deleted'",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "memory {} not found in trash",
            id
        )));
    }
    // Return the restored memory
    get(db, id, user_id).await
}

/// Permanently delete memories that have been in the trash longer than the
/// retention window (default 30 days). Returns the number of purged rows.
pub async fn purge_trashed(db: &Database, retention_days: i64) -> Result<usize> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM memories \
             WHERE is_forgotten = 1 \
               AND forget_reason = 'user_deleted' \
               AND updated_at < datetime('now', ?1)",
            rusqlite::params![format!("-{} days", retention_days)],
        )
        .map_err(rusqlite_to_eng_error)
    })
    .await
}

pub async fn update(db: &Database, id: i64, req: UpdateRequest, user_id: i64) -> Result<Memory> {
    // 1. Get the existing memory, scoped to user_id (outside transaction - read only)
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0",
        MEMORY_COLUMNS
    );

    let sql_for_read = sql.clone();
    let old = db
        .read(move |conn| {
            let mut stmt = conn.prepare(&sql_for_read).map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![id, user_id])
                .map_err(rusqlite_to_eng_error)?;
            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                row_to_memory(row)
            } else {
                Err(EngError::NotFound(format!("memory {} not found", id)))
            }
        })
        .await?;

    let new_content = req
        .content
        .as_deref()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| old.content.clone());

    if new_content.is_empty() {
        return Err(EngError::InvalidInput(
            "content cannot be empty".to_string(),
        ));
    }
    if new_content.len() > MAX_CONTENT_SIZE {
        return Err(EngError::InvalidInput(format!(
            "content exceeds maximum size of {} bytes",
            MAX_CONTENT_SIZE
        )));
    }

    let new_category = req.category.as_deref().unwrap_or(&old.category).to_string();
    let new_importance = clamp_importance(req.importance.unwrap_or(old.importance));
    let new_is_static = req.is_static.unwrap_or(old.is_static) as i32;
    let new_status = req.status.as_deref().unwrap_or(&old.status).to_string();
    let new_tags_json = if req.tags.is_some() {
        normalize_tags(&req.tags)
    } else {
        old.tags.clone()
    };
    let new_root_memory_id = old.root_memory_id.unwrap_or(old.id);
    let new_version = old.version + 1;

    let old_for_tx = old.clone();
    let embedding_for_tx = req.embedding.clone();
    let new_content_for_tx = new_content.clone();
    let new_category_for_tx = new_category.clone();
    let new_status_for_tx = new_status.clone();
    let new_tags_json_for_tx = new_tags_json.clone();

    let new_id = db
        .transaction(move |tx| {
            update_transactional_rusqlite(
                tx,
                id,
                user_id,
                &old_for_tx,
                &new_content_for_tx,
                &new_category_for_tx,
                new_importance,
                new_is_static,
                &new_status_for_tx,
                new_tags_json_for_tx,
                new_root_memory_id,
                new_version,
                embedding_for_tx.as_ref(),
            )
        })
        .await?;

    if let Some(ref emb) = req.embedding {
        if let Some(index) = db.vector_index.as_ref() {
            if let Err(e) = index.insert(new_id, user_id, emb).await {
                warn!("LanceDB vector insert failed for memory {}: {}", new_id, e);
                record_vector_sync_failure(db, new_id, user_id, "insert", &e.to_string()).await;
            }
            if let Err(e) = index.delete(id).await {
                warn!(
                    "LanceDB vector delete failed for superseded memory {}: {}",
                    id, e
                );
                record_vector_sync_failure(db, id, user_id, "delete", &e.to_string()).await;
            }
        }
    }

    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, user_id, 1).await {
        warn!("pagerank dirty mark failed on update: {}", e);
    }

    let new_sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2",
        MEMORY_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&new_sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![new_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            row_to_memory(row)
        } else {
            Err(EngError::Internal(
                "failed to fetch newly created memory version".to_string(),
            ))
        }
    })
    .await
}

/// Internal transactional helper for update - returns new_id.
///
/// The is_latest flip is guarded with `AND is_latest = 1` and the affected
/// row count is checked. If a concurrent update already superseded this
/// version the UPDATE matches 0 rows and we abort so the version chain
/// never ends up with two rows sharing `is_latest = 1` for the same root.
#[allow(clippy::too_many_arguments)]
fn update_transactional_rusqlite(
    tx: &rusqlite::Transaction<'_>,
    old_id: i64,
    user_id: i64,
    old: &Memory,
    new_content: &str,
    new_category: &str,
    new_importance: i32,
    new_is_static: i32,
    new_status: &str,
    new_tags_json: Option<String>,
    new_root_memory_id: i64,
    new_version: i32,
    embedding: Option<&Vec<f32>>,
) -> Result<i64> {
    let affected = tx
        .execute(
            "UPDATE memories SET is_latest = 0, updated_at = datetime('now') \
             WHERE id = ?1 AND user_id = ?2 AND is_latest = 1",
            rusqlite::params![old_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "memory {} is no longer the latest version (concurrent update)",
            old_id
        )));
    }

    tx.execute(
        "INSERT INTO memories (
            content, category, source, session_id, importance,
            version, is_latest, parent_memory_id, root_memory_id,
            is_static, tags, status, user_id, space_id,
            fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength,
            fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at,
            confidence, model
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, 1, ?7, ?8,
            ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17,
            ?18, ?19, ?20, ?21,
            ?22, ?23
        )",
        rusqlite::params![
            new_content,
            new_category,
            old.source.clone(),
            old.session_id.clone(),
            new_importance,
            new_version,
            old.id,
            new_root_memory_id,
            new_is_static,
            new_tags_json,
            new_status,
            old.user_id,
            old.space_id,
            old.fsrs_stability,
            old.fsrs_difficulty,
            old.fsrs_storage_strength,
            old.fsrs_retrieval_strength,
            old.fsrs_learning_state,
            old.fsrs_reps,
            old.fsrs_lapses,
            old.fsrs_last_review_at.clone(),
            old.confidence,
            old.model.clone()
        ],
    )
    .map_err(rusqlite_to_eng_error)?;

    let new_id = tx.last_insert_rowid();

    if let Some(emb) = embedding {
        let emb_blob = embedding_to_blob(emb);
        tx.execute(
            "UPDATE memories SET embedding_vec_1024 = ?1 WHERE id = ?2",
            rusqlite::params![emb_blob, new_id],
        )
        .map_err(rusqlite_to_eng_error)?;
    }

    Ok(new_id)
}

// -- Additional DB operations matching TS db.ts ---

pub async fn mark_forgotten(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            conn.execute(
                "UPDATE memories SET is_forgotten = 1, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    }
    if let Some(index) = db.vector_index.as_ref() {
        if let Err(e) = index.delete(id).await {
            warn!(
                "LanceDB vector delete failed for forgotten memory {}: {}",
                id, e
            );
            record_vector_sync_failure(db, id, user_id, "delete", &e.to_string()).await;
        }
    }
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, user_id, 1).await {
        warn!(
            "mark_pagerank_dirty failed after mark_forgotten for user {}: {}",
            user_id, e
        );
    }
    Ok(())
}

pub async fn mark_archived(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            conn.execute(
                "UPDATE memories SET is_archived = 1, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    }
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, user_id, 1).await {
        warn!(
            "mark_pagerank_dirty failed after mark_archived for user {}: {}",
            user_id, e
        );
    }
    Ok(())
}

pub async fn mark_unarchived(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            conn.execute(
                "UPDATE memories SET is_archived = 0, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    }
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, user_id, 1).await {
        warn!(
            "mark_pagerank_dirty failed after mark_unarchived for user {}: {}",
            user_id, e
        );
    }
    Ok(())
}

pub async fn update_forget_reason(
    db: &Database,
    id: i64,
    reason: &str,
    user_id: i64,
) -> Result<()> {
    let reason = reason.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET forget_reason = ?1 WHERE id = ?2 AND user_id = ?3",
            rusqlite::params![reason, id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;
    Ok(())
}

pub async fn adjust_importance(
    db: &Database,
    memory_id: i64,
    user_id: i64,
    delta: i32,
) -> Result<()> {
    db.write(move |conn| {
        let sql = if delta > 0 {
            "UPDATE memories SET importance = MIN(importance + ?1, 10) WHERE id = ?2 AND user_id = ?3"
        } else {
            "UPDATE memories SET importance = MAX(importance + ?1, 0) WHERE id = ?2 AND user_id = ?3"
        };
        conn.execute(sql, rusqlite::params![delta, memory_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn insert_link(
    db: &Database,
    source_id: i64,
    target_id: i64,
    similarity: f64,
    link_type: &str,
    user_id: i64,
) -> Result<()> {
    // Validate both memories belong to this user before inserting the link
    let count_sql =
        "SELECT COUNT(*) FROM memories WHERE id IN (?1, ?2) AND user_id = ?3 AND is_forgotten = 0";
    let link_type = link_type.to_string();
    db.write(move |conn| {
        let count: i64 = conn
            .query_row(
                count_sql,
                rusqlite::params![source_id, target_id, user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;
        if count < 2 {
            return Err(EngError::NotFound(format!(
                "one or both memories ({}, {}) do not belong to user {}",
                source_id, target_id, user_id
            )));
        }
        conn.execute(
            "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![source_id, target_id, similarity, link_type],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, user_id, 1).await {
        warn!(
            "mark_pagerank_dirty failed after insert_link for user {}: {}",
            user_id, e
        );
    }
    Ok(())
}

pub async fn update_source_count(
    db: &Database,
    id: i64,
    source_count: i32,
    user_id: i64,
) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET source_count = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
            rusqlite::params![source_count, id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

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
        if count.is_multiple_of(1000) {
            tracing::info!(count, "rebuilt LanceDB vector index rows");
        }
    }

    Ok(count)
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct VectorSyncReplayReport {
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
}

/// Drain the vector_sync_pending ledger. For each row, retry the failed
/// LanceDB op and remove the row on success. Rows whose underlying memory
/// no longer has an embedding (or has been hard-deleted) are considered
/// skipped and also removed.
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

pub async fn list_all_tags(db: &Database, user_id: i64) -> Result<Vec<TagCount>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT tags FROM memories
                 WHERE user_id = ?1
                   AND is_forgotten = 0
                   AND is_latest = 1
                   AND tags IS NOT NULL
                   AND tags != '[]'",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;

        let mut counts: HashMap<String, i64> = HashMap::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let raw_tags: Option<String> = row.get(0).map_err(rusqlite_to_eng_error)?;
            for tag in parse_tags_json(&raw_tags) {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }

        let mut tags: Vec<TagCount> = counts
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect();
        tags.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.tag.cmp(&b.tag)));
        Ok(tags)
    })
    .await
}

pub async fn search_by_tags(
    db: &Database,
    user_id: i64,
    tags: &[String],
    match_all: bool,
    limit: usize,
) -> Result<Vec<Memory>> {
    let normalized: Vec<String> = tags
        .iter()
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();

    if normalized.is_empty() {
        return Ok(Vec::new());
    }

    let wanted: HashSet<String> = normalized.iter().cloned().collect();
    let sql = format!(
        "SELECT {} FROM memories
         WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
         ORDER BY created_at DESC",
        MEMORY_COLUMNS
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut memories = Vec::new();

        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let memory = row_to_memory(row)?;
            let memory_tags: HashSet<String> = parse_tags_json(&memory.tags).into_iter().collect();
            let matched = if match_all {
                wanted.iter().all(|tag| memory_tags.contains(tag))
            } else {
                wanted.iter().any(|tag| memory_tags.contains(tag))
            };

            if matched {
                memories.push(memory);
                if memories.len() >= limit {
                    break;
                }
            }
        }

        Ok(memories)
    })
    .await
}

pub async fn update_memory_tags(
    db: &Database,
    memory_id: i64,
    user_id: i64,
    tags: &[String],
) -> Result<()> {
    let _ = get(db, memory_id, user_id).await?;
    let normalized = if tags.is_empty() {
        None
    } else {
        let owned_tags = Some(tags.to_vec());
        normalize_tags(&owned_tags)
    };

    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET tags = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
            rusqlite::params![normalized, memory_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn get_links_for(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<LinkedMemory>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT ml.target_id, ml.similarity, ml.type,
                        m.content, m.category, m.is_forgotten
                 FROM memory_links ml
                 JOIN memories m ON m.id = ml.target_id
                 WHERE ml.source_id = ?1 AND m.user_id = ?2
                 UNION
                 SELECT ml.source_id, ml.similarity, ml.type,
                        m.content, m.category, m.is_forgotten
                 FROM memory_links ml
                 JOIN memories m ON m.id = ml.source_id
                 WHERE ml.target_id = ?1 AND m.user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![memory_id, user_id])
            .map_err(rusqlite_to_eng_error)?;

        let mut links = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            if row.get::<_, i32>(5).map_err(rusqlite_to_eng_error)? != 0 {
                continue;
            }
            links.push(LinkedMemory {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                similarity: row.get(1).map_err(rusqlite_to_eng_error)?,
                link_type: row.get(2).map_err(rusqlite_to_eng_error)?,
                content: row.get(3).map_err(rusqlite_to_eng_error)?,
                category: row.get(4).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(links)
    })
    .await
}

pub async fn get_version_chain(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<VersionChainEntry>> {
    let memory = get(db, memory_id, user_id).await?;
    let root_id = memory.root_memory_id.unwrap_or(memory.id);

    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, content, version, is_latest
                 FROM memories
                 WHERE (root_memory_id = ?1 OR id = ?1)
                   AND user_id = ?2
                 ORDER BY version ASC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![root_id, user_id])
            .map_err(rusqlite_to_eng_error)?;

        let mut chain = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            chain.push(VersionChainEntry {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                content: row.get(1).map_err(rusqlite_to_eng_error)?,
                version: row.get(2).map_err(rusqlite_to_eng_error)?,
                is_latest: row.get::<_, i32>(3).map_err(rusqlite_to_eng_error)? != 0,
            });
        }
        Ok(chain)
    })
    .await
}

async fn count_user_rows(db: &Database, sql: &str, user_id: i64) -> Result<i64> {
    let sql = sql.to_string();
    db.read(move |conn| {
        conn.query_row(&sql, rusqlite::params![user_id], |row| row.get(0))
            .map_err(rusqlite_to_eng_error)
    })
    .await
}

pub async fn get_user_profile(db: &Database, user_id: i64) -> Result<UserProfile> {
    let (memory_count, oldest_memory, newest_memory, avg_importance) = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*), MIN(created_at), MAX(created_at), AVG(importance)
                 FROM memories
                 WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
                rusqlite::params![user_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<f64>>(3)?.unwrap_or(0.0),
                    ))
                },
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;

    let top_categories = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT category, COUNT(*)
                     FROM memories
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
                     GROUP BY category
                     ORDER BY COUNT(*) DESC, category ASC
                     LIMIT 10",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![user_id])
                .map_err(rusqlite_to_eng_error)?;

            let mut top_categories = Vec::new();
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                top_categories.push(CategoryCount {
                    category: row.get(0).map_err(rusqlite_to_eng_error)?,
                    count: row.get(1).map_err(rusqlite_to_eng_error)?,
                });
            }
            Ok(top_categories)
        })
        .await?;

    let top_tags = list_all_tags(db, user_id)
        .await?
        .into_iter()
        .take(10)
        .collect();
    let personality_traits = personality::get_profile(db, user_id)
        .await
        .map(|profile| profile.traits)
        .unwrap_or_else(|_| serde_json::json!({}));
    let personality_summary = personality::get_profile_for_injection(db, user_id)
        .await?
        .and_then(|(profile, _)| {
            let trimmed = profile.trim().to_string();
            if trimmed.is_empty() || trimmed == "{}" {
                None
            } else {
                Some(trimmed)
            }
        });

    Ok(UserProfile {
        user_id,
        personality_traits,
        personality_summary,
        memory_count,
        oldest_memory,
        newest_memory,
        avg_importance,
        top_categories,
        top_tags,
    })
}

pub async fn get_user_stats(db: &Database, user_id: i64) -> Result<UserStats> {
    let memories = count_user_rows(
        db,
        "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
        user_id,
    )
    .await?;
    let archived = count_user_rows(
        db,
        "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_archived = 1 AND is_latest = 1",
        user_id,
    )
    .await?;
    let conversations = count_user_rows(
        db,
        "SELECT COUNT(*) FROM conversations WHERE user_id = ?1",
        user_id,
    )
    .await?;
    let episodes = count_user_rows(
        db,
        "SELECT COUNT(*) FROM episodes WHERE user_id = ?1",
        user_id,
    )
    .await?;
    let entities = count_user_rows(
        db,
        "SELECT COUNT(*) FROM entities WHERE user_id = ?1",
        user_id,
    )
    .await?;
    let skills = count_user_rows(
        db,
        "SELECT COUNT(*) FROM skill_records WHERE user_id = ?1",
        user_id,
    )
    .await?;

    let categories = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT category, COUNT(*)
                     FROM memories
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
                     GROUP BY category
                     ORDER BY COUNT(*) DESC, category ASC",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![user_id])
                .map_err(rusqlite_to_eng_error)?;
            let mut categories = BTreeMap::new();
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                categories.insert(
                    row.get::<_, String>(0).map_err(rusqlite_to_eng_error)?,
                    row.get::<_, i64>(1).map_err(rusqlite_to_eng_error)?,
                );
            }
            Ok(categories)
        })
        .await?;

    Ok(UserStats {
        memories,
        archived,
        conversations,
        episodes,
        entities,
        skills,
        categories,
    })
}
