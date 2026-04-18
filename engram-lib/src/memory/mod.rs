//! Memory domain -- storage, retrieval, and lifecycle for the core `memories` table.
//!
//! Submodules:
//! - [`search`]       hybrid search (FTS + vector + graph), faceted search, RRF fusion.
//! - [`fts`]          SQLite FTS5 helpers and tokenization.
//! - [`vector`]       vector-search helpers over the LanceDB embeddings index.
//! - [`vector_sync`]  backfill + replay of the `vector_sync_pending` ledger.
//! - [`scoring`]      decay, pagerank, and per-channel scoring utilities.
//! - [`simhash`]      near-duplicate detection via SimHash / Hamming buckets.
//! - [`types`]        request/response DTOs, `Memory`, `SearchResult`.
//!
//! This module (`mod.rs`) owns the CRUD surface: `store`, `get`, `list`,
//! `update`, `delete`, plus tag/version helpers. Search lives in `search.rs`.
//! The public `MEMORY_COLUMNS` constant and `row_to_memory` helper keep the
//! SELECT shape and row-to-struct mapping in sync -- see the guard tests at
//! the bottom of this file.

pub mod fts;
pub mod scoring;
pub mod search;
pub mod simhash;
pub mod types;
pub mod vector;
pub mod vector_sync;

use crate::db::Database;
use crate::personality;
use crate::EngError;
use crate::Result;
use std::collections::{BTreeMap, HashMap};
use tracing::warn;
use types::{
    CategoryCount, LinkedMemory, ListOptions, Memory, StoreRequest, StoreResult, TagCount,
    UpdateRequest, UserProfile, UserStats, VersionChainEntry,
};

pub use vector_sync::{
    build_lance_index_from_existing, replay_vector_sync_pending,
    replay_vector_sync_pending_for_user, vector_sync_pending_users, VectorSyncReplayReport,
};

// -- Constants ---

use crate::validation::MAX_CONTENT_SIZE;

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

/// Number of columns in `MEMORY_COLUMNS`. Must match the highest index
/// `row_to_memory` reads from (indices 0..MEMORY_COLUMN_COUNT-1).
pub(crate) const MEMORY_COLUMN_COUNT: usize = 48;

// -- Public CRUD functions ---

#[tracing::instrument(skip(db, req), fields(user_id = req.user_id.unwrap_or(0), content_len = req.content.len()))]
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

    // Compute and persist emotional valence for future affect-weighted retrieval.
    // Best-effort: a failure here must not block the store.
    if let Err(e) = crate::intelligence::valence::store_valence(db, new_id, &content, user_id).await
    {
        warn!("valence analysis failed for memory {}: {}", new_id, e);
    }

    search::invalidate_search_cache(user_id);

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
#[tracing::instrument(skip(db))]
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
#[tracing::instrument(skip(db))]
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

#[tracing::instrument(skip(db, opts), fields(user_id = opts.user_id.unwrap_or(0), limit = opts.limit))]
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

    // 6.9 capacity hint: LIMIT bounds the row count.
    let cap = opts.limit;
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let rusqlite_params = rusqlite::params_from_iter(param_values.iter().cloned());
        let mut rows = stmt.query(rusqlite_params).map_err(rusqlite_to_eng_error)?;
        let mut memories = Vec::with_capacity(cap);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            memories.push(row_to_memory(row)?);
        }
        Ok(memories)
    })
    .await
}

#[tracing::instrument(skip(db))]
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
    search::invalidate_search_cache(user_id);
    Ok(())
}

/// List soft-deleted memories for a user (recovery window).
/// Only returns memories deleted by the user (`forget_reason = 'user_deleted'`),
/// not system-initiated forgets (consolidation, contradiction, etc.).
#[tracing::instrument(skip(db))]
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
        // 6.9 capacity hint: LIMIT bounds the row count.
        let mut result = Vec::with_capacity(limit);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            result.push(row_to_memory(row)?);
        }
        Ok(result)
    })
    .await
}

/// Restore a soft-deleted memory (undo user delete).
/// Returns the restored memory. Fails if the memory is not in a user-deleted state.
#[tracing::instrument(skip(db))]
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
    search::invalidate_search_cache(user_id);
    // Return the restored memory
    get(db, id, user_id).await
}

/// Permanently delete memories that have been in the trash longer than the
/// retention window (default 30 days). Returns the number of purged rows.
#[tracing::instrument(skip(db))]
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

#[tracing::instrument(skip(db, req))]
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
    search::invalidate_search_cache(user_id);

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

#[tracing::instrument(skip(db))]
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
    search::invalidate_search_cache(user_id);
    Ok(())
}

#[tracing::instrument(skip(db))]
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
    search::invalidate_search_cache(user_id);
    Ok(())
}

#[tracing::instrument(skip(db))]
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
    search::invalidate_search_cache(user_id);
    Ok(())
}

#[tracing::instrument(skip(db, reason))]
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

#[tracing::instrument(skip(db))]
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

#[tracing::instrument(skip(db))]
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
    search::invalidate_search_cache(user_id);
    Ok(())
}

#[tracing::instrument(skip(db))]
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

#[tracing::instrument(skip(db))]
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

#[tracing::instrument(skip(db, tags), fields(tag_count = tags.len()))]
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

    // Use json_each() to push tag filtering to SQL level instead of
    // scanning every row and deserializing in Rust.
    let tag_count = normalized.len();
    let placeholders: Vec<String> = (0..tag_count).map(|i| format!("?{}", i + 2)).collect();

    let sql = if match_all {
        // match_all: memory must contain ALL requested tags.
        // Count distinct matches from json_each; must equal tag_count.
        format!(
            "SELECT {} FROM memories m
             WHERE m.user_id = ?1
               AND m.is_forgotten = 0
               AND m.is_latest = 1
               AND m.tags IS NOT NULL
               AND (SELECT COUNT(DISTINCT je.value)
                    FROM json_each(m.tags) je
                    WHERE je.value IN ({})) = {}
             ORDER BY m.created_at DESC
             LIMIT {}",
            MEMORY_COLUMNS,
            placeholders.join(", "),
            tag_count,
            limit
        )
    } else {
        // match_any: memory must contain at least one requested tag.
        format!(
            "SELECT {} FROM memories m
             WHERE m.user_id = ?1
               AND m.is_forgotten = 0
               AND m.is_latest = 1
               AND m.tags IS NOT NULL
               AND EXISTS (
                   SELECT 1 FROM json_each(m.tags) je
                   WHERE je.value IN ({})
               )
             ORDER BY m.created_at DESC
             LIMIT {}",
            MEMORY_COLUMNS,
            placeholders.join(", "),
            limit
        )
    };

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        // Bind user_id at index 1, then each tag at indices 2..N+1.
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            Vec::with_capacity(tag_count + 1);
        params_vec.push(Box::new(user_id));
        for tag in &normalized {
            params_vec.push(Box::new(tag.clone()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut rows = stmt
            .query(param_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;
        // 6.9 capacity hint: LIMIT bounds the row count.
        let mut memories = Vec::with_capacity(limit);
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            memories.push(row_to_memory(row)?);
        }
        Ok(memories)
    })
    .await
}

#[tracing::instrument(skip(db, tags), fields(tag_count = tags.len()))]
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
    .await?;
    search::invalidate_search_cache(user_id);
    Ok(())
}

#[tracing::instrument(skip(db))]
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

        // 6.9 capacity hint: link fanout typically small.
        let mut links = Vec::with_capacity(16);
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

#[tracing::instrument(skip(db))]
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

        // 6.9 capacity hint: version chains are usually short.
        let mut chain = Vec::with_capacity(8);
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

#[tracing::instrument(skip(db))]
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

            // 6.9 capacity hint: SQL caps at LIMIT 10.
            let mut top_categories = Vec::with_capacity(10);
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

#[tracing::instrument(skip(db))]
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard: MEMORY_COLUMNS and row_to_memory must stay aligned. If this test
    /// fails, either the SELECT column list or the row_to_memory mapping
    /// drifted -- both must be updated together.
    #[test]
    fn memory_columns_count_matches_row_mapping() {
        let n = MEMORY_COLUMNS
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .count();
        assert_eq!(
            n, MEMORY_COLUMN_COUNT,
            "MEMORY_COLUMNS has {n} columns; MEMORY_COLUMN_COUNT says {MEMORY_COLUMN_COUNT}. \
             Update row_to_memory mapping and MEMORY_COLUMN_COUNT together."
        );
    }

    /// Guard: the SELECT list must pull columns from `memories` with the same
    /// order that `row_to_memory` reads by index. A live in-memory DB is the
    /// cheapest way to catch typos (renamed column, wrong name) without
    /// waiting for a runtime hit.
    #[tokio::test]
    async fn memory_columns_match_schema_for_select() {
        use rusqlite::params;
        let db = Database::connect_memory().await.expect("in-mem db");
        db.write(|conn| {
            conn.execute(
                "INSERT INTO memories (content, category, source, user_id, importance, confidence, \
                 created_at, updated_at, is_latest, is_forgotten, is_archived) \
                 VALUES ('col-audit', 'general', 'test', 1, 5, 1.0, \
                 datetime('now'), datetime('now'), 1, 0, 0)",
                params![],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await
        .expect("seed");

        let sql = format!("SELECT {MEMORY_COLUMNS} FROM memories LIMIT 1");
        let got = db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                let mem = stmt
                    .query_row(params![], |row| {
                        row_to_memory(row).map_err(|e| {
                            rusqlite::Error::ToSqlConversionFailure(Box::new(
                                std::io::Error::other(e.to_string()),
                            ))
                        })
                    })
                    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
                Ok(mem)
            })
            .await
            .expect("select");
        assert_eq!(got.content, "col-audit");
    }

    fn valence_store_request(content: &str, user_id: i64) -> crate::memory::types::StoreRequest {
        crate::memory::types::StoreRequest {
            content: content.to_string(),
            category: "test".to_string(),
            source: "test".to_string(),
            importance: 5,
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            user_id: Some(user_id),
            space_id: None,
            parent_memory_id: None,
        }
    }

    async fn read_valence(db: &Database, id: i64) -> (Option<f64>, Option<String>) {
        db.read(move |conn| {
            conn.query_row(
                "SELECT valence, dominant_emotion FROM memories WHERE id = ?1",
                rusqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("read valence")
    }

    #[tokio::test]
    async fn store_persists_positive_valence_for_happy_content() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let user_id = 1;
        let stored = store(
            &db,
            valence_store_request("I am ecstatic and thrilled about this release", user_id),
        )
        .await
        .expect("store");
        let (valence, emotion) = read_valence(&db, stored.id).await;
        assert!(valence.unwrap_or(0.0) > 0.5, "valence should be positive");
        assert_ne!(emotion.unwrap_or_default(), "");
    }

    #[tokio::test]
    async fn store_persists_negative_valence_for_angry_content() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let user_id = 1;
        let stored = store(
            &db,
            valence_store_request("I am furious and everything crashed", user_id),
        )
        .await
        .expect("store");
        let (valence, emotion) = read_valence(&db, stored.id).await;
        assert!(valence.unwrap_or(0.0) < 0.0, "valence should be negative");
        assert_ne!(emotion.unwrap_or_default(), "");
    }

    #[tokio::test]
    async fn store_leaves_valence_null_for_neutral_content() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let user_id = 1;
        let stored = store(
            &db,
            valence_store_request("The meeting is at 3pm tomorrow", user_id),
        )
        .await
        .expect("store");
        let (valence, emotion) = read_valence(&db, stored.id).await;
        assert_eq!(valence, None, "neutral content leaves valence null");
        assert_eq!(emotion, None);
    }
}
