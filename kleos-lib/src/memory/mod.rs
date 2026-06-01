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

pub mod auto_tag;
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

pub use types::VectorSyncReplayReport;
pub use vector_sync::{
    backfill_missing_embeddings, backfill_missing_embeddings_limited,
    build_lance_chunk_index_from_existing, build_lance_index_from_existing,
    replay_vector_sync_pending, replay_vector_sync_pending_for_user, vector_sync_pending_users,
    BackfillReport,
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

/// Parse a stored JSON tag list, returning an empty list for absent or invalid tags.
fn parse_tags_json(tags: &Option<String>) -> Vec<String> {
    tags.as_ref()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
        .unwrap_or_default()
}

/// Clamp user-provided importance into the supported memory range.
fn clamp_importance(value: i32) -> i32 {
    value.clamp(1, 10)
}

/// Record a failed LanceDB write into the vector_sync_pending table so a
/// sweeper (or the admin replay endpoint) can retry it. Intentionally
/// best-effort: if the sync-pending insert itself fails, log and move on.
async fn record_vector_sync_failure(
    db: &Database,
    memory_id: i64,
    _user_id: i64,
    op: &str,
    err: &str,
) {
    let op_owned = op.to_string();
    let err_owned = err.to_string();
    let op_for_log = op_owned.clone();
    let result = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO vector_sync_pending (memory_id, op, error) \
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![memory_id, op_owned, err_owned],
            )?;
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

/// Replace stored chunk rows and chunk vectors for one memory.
pub async fn write_chunks(db: &Database, memory_id: i64, chunks: &[(String, Vec<f32>)]) {
    let chunks_for_tx: Vec<(String, Vec<u8>)> = chunks
        .iter()
        .map(|(text, emb)| (text.clone(), embedding_to_blob(emb)))
        .collect();

    let result = db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM memory_chunks WHERE memory_id = ?1",
                rusqlite::params![memory_id],
            )?;

            let mut stmt = conn.prepare(
                "INSERT INTO memory_chunks (memory_id, chunk_idx, content, embedding_vec_1024) \
                     VALUES (?1, ?2, ?3, ?4)",
            )?;

            for (idx, (text, blob)) in chunks_for_tx.iter().enumerate() {
                stmt.execute(rusqlite::params![memory_id, idx as i64, text, blob])?;
            }
            Ok(())
        })
        .await;

    if let Err(e) = result {
        warn!("chunk row write failed for memory {}: {}", memory_id, e);
    }

    if let Some(index) = db.chunk_vector_index.as_ref() {
        let batch: Vec<(i64, Vec<f32>)> = chunks
            .iter()
            .enumerate()
            .map(|(idx, (_, emb))| (chunk_lance_key(memory_id, idx), emb.clone()))
            .collect();
        if let Err(e) = index.insert_many(&batch).await {
            warn!(
                "LanceDB chunk vector batch insert failed for memory {}: {}",
                memory_id, e
            );
        }
    }
}

/// Copy chunk rows and vectors from an old memory version to a new version.
async fn carry_forward_chunks(db: &Database, old_memory_id: i64, new_memory_id: i64) {
    let result = db
        .write(move |conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM memory_chunks WHERE memory_id = ?1",
                rusqlite::params![old_memory_id],
                |row| row.get(0),
            )?;

            if count == 0 {
                return Ok(());
            }

            conn.execute(
                "INSERT INTO memory_chunks (memory_id, chunk_idx, content, embedding_vec_1024)
                 SELECT ?1, chunk_idx, content, embedding_vec_1024
                 FROM memory_chunks WHERE memory_id = ?2",
                rusqlite::params![new_memory_id, old_memory_id],
            )?;

            Ok(())
        })
        .await;

    if let Err(e) = result {
        warn!(
            "carry_forward_chunks failed {} -> {}: {}",
            old_memory_id, new_memory_id, e
        );
        return;
    }

    if let Some(index) = db.chunk_vector_index.as_ref() {
        let chunks: Vec<(usize, Vec<f32>)> = match db
            .read(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT chunk_idx, embedding_vec_1024 FROM memory_chunks \
                         WHERE memory_id = ?1 ORDER BY chunk_idx",
                )?;
                let rows = stmt.query_map(rusqlite::params![new_memory_id], |row| {
                    let idx: usize = row.get(0)?;
                    let blob: Option<Vec<u8>> = row.get(1)?;
                    Ok((idx, blob))
                })?;
                let mut out = Vec::new();
                for r in rows {
                    let (idx, blob) = r?;
                    if let Some(b) = blob {
                        let emb: Vec<f32> = b
                            .chunks_exact(4)
                            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                            .collect();
                        out.push((idx, emb));
                    }
                }
                Ok(out)
            })
            .await
        {
            Ok(c) => c,
            Err(e) => {
                warn!("reading carried-forward chunks for LanceDB: {}", e);
                return;
            }
        };

        let batch: Vec<(i64, Vec<f32>)> = chunks
            .iter()
            .map(|(idx, emb)| (chunk_lance_key(new_memory_id, *idx), emb.clone()))
            .collect();
        if let Err(e) = index.insert_many(&batch).await {
            warn!(
                "LanceDB chunk carry-forward batch insert failed for memory {}: {}",
                new_memory_id, e
            );
        }

        // Clean up old memory's chunk vectors
        let old_chunks: Vec<usize> = chunks.iter().map(|(idx, _)| *idx).collect();
        for idx in old_chunks {
            let key = chunk_lance_key(old_memory_id, idx);
            let _ = index.delete(key).await;
        }
    }
}

/// Encode a memory id and chunk index into the LanceDB chunk key space.
fn chunk_lance_key(memory_id: i64, chunk_idx: usize) -> i64 {
    memory_id * 1000 + chunk_idx as i64
}

/// Decode a LanceDB chunk key back to the owning memory id.
pub fn lance_key_to_memory_id(chunk_key: i64) -> i64 {
    chunk_key / 1000
}

/// Serialize a normalized embedding into a little-endian byte blob.
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
///   episode_id, decay_score, confidence, sync_id, status, space_id,
///   fsrs_stability, fsrs_difficulty, fsrs_storage_strength,
///   fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses,
///   fsrs_last_review_at, valence, arousal, dominant_emotion,
///   created_at, updated_at, is_superseded, is_consolidated
///
/// `owner_user_id` is supplied by the caller (from function parameter) because
/// user_id is no longer a column in MEMORY_COLUMNS -- the field is populated
/// from context rather than from the row.
pub(crate) fn row_to_memory(row: &rusqlite::Row<'_>, owner_user_id: i64) -> Result<Memory> {
    Ok(Memory {
        id: row.get(0)?,
        content: row.get(1)?,
        category: row.get(2)?,
        source: row.get(3)?,
        session_id: row.get(4)?,
        importance: row.get(5)?,
        embedding: None,
        version: row.get(6)?,
        is_latest: row.get::<_, i32>(7)? != 0,
        parent_memory_id: row.get(8)?,
        root_memory_id: row.get(9)?,
        source_count: row.get(10)?,
        is_static: row.get::<_, i32>(11)? != 0,
        is_forgotten: row.get::<_, i32>(12)? != 0,
        is_archived: row.get::<_, i32>(13)? != 0,
        is_fact: row.get::<_, i32>(14)? != 0,
        is_decomposed: row.get::<_, i32>(15)? != 0,
        forget_after: row.get(16)?,
        forget_reason: row.get(17)?,
        model: row.get(18)?,
        recall_hits: row.get(19)?,
        recall_misses: row.get(20)?,
        adaptive_score: row.get(21)?,
        pagerank_score: row.get(22)?,
        last_accessed_at: row.get(23)?,
        access_count: row.get(24)?,
        tags: row.get(25)?,
        episode_id: row.get(26)?,
        decay_score: row.get(27)?,
        confidence: row.get(28)?,
        sync_id: row.get(29)?,
        status: row.get(30)?,
        // user_id is no longer a column in MEMORY_COLUMNS; filled from caller.
        user_id: owner_user_id,
        space_id: row.get(31)?,
        fsrs_stability: row.get(32)?,
        fsrs_difficulty: row.get(33)?,
        fsrs_storage_strength: row.get(34)?,
        fsrs_retrieval_strength: row.get(35)?,
        fsrs_learning_state: row.get(36)?,
        fsrs_reps: row.get(37)?,
        fsrs_lapses: row.get(38)?,
        fsrs_last_review_at: row.get(39)?,
        valence: row.get(40)?,
        arousal: row.get(41)?,
        dominant_emotion: row.get(42)?,
        created_at: row.get(43)?,
        updated_at: row.get(44)?,
        is_superseded: row.get::<_, i32>(45)? != 0,
        is_consolidated: row.get::<_, i32>(46)? != 0,
    })
}

/// Standard SELECT column list -- matches row_to_memory index order.
/// Note: user_id is NOT listed here; it is supplied to row_to_memory as the
/// `owner_user_id` argument from caller context (Phase 5.1).
pub(crate) const MEMORY_COLUMNS: &str = "id, content, category, source, session_id, importance, \
    version, is_latest, parent_memory_id, root_memory_id, source_count, is_static, \
    is_forgotten, is_archived, is_fact, is_decomposed, \
    forget_after, forget_reason, model, recall_hits, recall_misses, \
    adaptive_score, pagerank_score, last_accessed_at, access_count, tags, \
    episode_id, decay_score, confidence, sync_id, status, space_id, \
    fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, \
    fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, \
    valence, arousal, dominant_emotion, created_at, updated_at, is_superseded, is_consolidated";

/// Number of columns in `MEMORY_COLUMNS`. Must match the highest index
/// `row_to_memory` reads from (indices 0..MEMORY_COLUMN_COUNT-1). Consumed
/// only by the test guard below; a non-test reference would be redundant
/// with the SELECT list itself.
#[cfg(test)]
pub(crate) const MEMORY_COLUMN_COUNT: usize = 47;

// -- Public CRUD functions ---

/// Store a memory, computing chunk embeddings via `embedder` when the
/// caller hasn't pre-supplied them. This is the path callers should use
/// when they already hold an embedder reference (HTTP routes, ingestion,
/// MCP tools, harness-seed). Internal subsystems without an embedder
/// reference (intelligence/*, jobs/*) call `store` directly and rely on
/// the admin backfill route to populate chunks later.
#[tracing::instrument(skip(db, embedder, req), fields(user_id = req.user_id.unwrap_or(0), content_len = req.content.len()))]
pub async fn store_with_chunks(
    db: &Database,
    embedder: &dyn crate::embeddings::EmbeddingProvider,
    mut req: StoreRequest,
) -> Result<StoreResult> {
    if req.embedding.is_none() {
        match embedder.embed(&req.content).await {
            Ok(emb) => req.embedding = Some(emb),
            Err(e) => tracing::warn!("embedding failed in store_with_chunks: {}", e),
        }
    }
    if req.chunk_embeddings.is_none() {
        match crate::embeddings::chunking::chunk_and_embed(
            embedder,
            &req.content,
            db.embedding_chunk_max_chars,
            db.embedding_chunk_overlap,
            db.embedding_chunk_max_chunks,
        )
        .await
        {
            Ok(pairs) if !pairs.is_empty() => req.chunk_embeddings = Some(pairs),
            Ok(_) => {}
            Err(e) => tracing::warn!("chunk embedding failed in store_with_chunks: {}", e),
        }
    }
    store(db, req, None, false).await
}

/// Store a single memory entry, enforcing content constraints and optional tenant quota.
///
/// `tenant_quota` -- when `Some`, the write is gated by an atomic in-transaction
/// quota check (E2 disk-quota path). Pass `None` for internal/background callers
/// that are not subject to per-tenant limits.
///
/// `shard_read_only` -- when `true` the function returns `EngError::QuotaExceeded`
/// immediately (E2 fast-path: shard has already exceeded disk quota).
#[tracing::instrument(skip(db, req), fields(user_id = req.user_id.unwrap_or(0), content_len = req.content.len()))]
pub async fn store(
    db: &Database,
    mut req: StoreRequest,
    tenant_quota: Option<std::sync::Arc<crate::tenant::types::QuotaConfig>>,
    shard_read_only: bool,
) -> Result<StoreResult> {
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

    // E2: disk quota fast-path -- shard is in read-only mode.
    if shard_read_only {
        return Err(EngError::QuotaExceeded(
            "tenant shard is in read-only mode (disk quota exceeded)".into(),
        ));
    }

    // SEC-recall-1.8: L2-normalize the embedding before any downstream use so
    // cosine semantics are correct regardless of provider. OnnxProvider
    // already normalizes its output; HttpProvider and OpenAiProvider do not.
    // `l2_normalize` is idempotent for unit-norm input and zero-vector safe.
    if let Some(ref mut emb) = req.embedding {
        crate::embeddings::normalize::l2_normalize(emb);
    }

    // SECURITY: previous code defaulted to user 1 (typically the bootstrap
    // admin). A caller that forgot to set user_id would silently attribute
    // the memory to tenant 1 -- fail closed instead.
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id required".into()))?;

    let importance = clamp_importance(req.importance);

    // Byte length of the trimmed content, used for E2 quota counter updates.
    let content_bytes = content.len() as i64;

    // 2. Compute simhash of content
    let content_hash = simhash::simhash(&content);

    // 3. Check for duplicates within the owner's own memories. The user_id
    // predicate keeps single-DB (shared) mode from deduping one user's write
    // against another user's content (and from leaking the other id back).
    let dup_sql = "SELECT id, content FROM memories \
        WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1 AND is_consolidated = 0 \
        ORDER BY id DESC LIMIT 1000";

    let duplicate = db
        .read(move |conn| {
            let mut stmt = conn.prepare(dup_sql)?;
            let mut rows = stmt.query(rusqlite::params![user_id])?;
            while let Some(row) = rows.next()? {
                let existing_id: i64 = row.get(0)?;
                let existing_content: String = row.get(1)?;
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

    // Auto-tag if tags empty
    let tags_json = {
        let has_tags = req.tags.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
        if has_tags {
            normalize_tags(&req.tags)
        } else {
            let inferred = auto_tag::infer_tags(&content);
            if inferred.is_empty() {
                None
            } else {
                normalize_tags(&Some(inferred))
            }
        }
    };

    // Auto-categorize if general
    let category = if req.category == "general" {
        auto_tag::infer_category(&content)
            .unwrap_or("general")
            .to_string()
    } else {
        req.category.clone()
    };

    let content_for_tx = content.clone();
    let req_for_tx = req.clone();
    let tags_json_for_tx = tags_json.clone();
    let category_for_tx = category.clone();
    let quota_for_tx = tenant_quota.clone();
    let content_bytes_for_tx = content_bytes;

    let new_id = db
        .transaction(move |tx| {
            // E2: atomic content quota check inside the writer-serialized transaction.
            if let Some(ref q) = quota_for_tx {
                crate::quota::enforce_quota_in_tx(tx, q, content_bytes_for_tx)?;
            }
            let id = store_transactional_rusqlite(
                tx,
                &content_for_tx,
                &req_for_tx,
                user_id,
                importance,
                tags_json_for_tx,
                &category_for_tx,
            )?;
            // E2: increment counters atomically in the same transaction.
            if quota_for_tx.is_some() {
                tx.execute(
                    "UPDATE tenant_state SET value = value + ?1, \
                     updated_at = datetime('now') \
                     WHERE key = 'content_bytes'",
                    rusqlite::params![content_bytes_for_tx],
                )
                .map_err(|e| {
                    crate::EngError::DatabaseMessage(format!("counter update failed: {e}"))
                })?;
                tx.execute(
                    "UPDATE tenant_state SET value = value + 1, \
                     updated_at = datetime('now') \
                     WHERE key = 'memory_count'",
                    [],
                )
                .map_err(|e| {
                    crate::EngError::DatabaseMessage(format!("counter update failed: {e}"))
                })?;
            }
            Ok(id)
        })
        .await?;

    if let Some(ref emb) = req.embedding {
        if let Some(index) = db.vector_index.as_ref() {
            if let Err(e) = index.insert(new_id, emb).await {
                warn!("LanceDB vector insert failed for memory {}: {}", new_id, e);
                record_vector_sync_failure(db, new_id, user_id, "insert", &e.to_string()).await;
            }
        }
    }

    if let Some(ref chunks) = req.chunk_embeddings {
        write_chunks(db, new_id, chunks).await;
    }

    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, 1).await {
        warn!("pagerank dirty mark failed on store: {}", e);
    }

    // Compute and persist emotional valence for future affect-weighted retrieval.
    // Best-effort: a failure here must not block the store.
    if let Err(e) = crate::intelligence::valence::store_valence(db, new_id, &content).await {
        warn!("valence analysis failed for memory {}: {}", new_id, e);
    }

    search::invalidate_search_cache(user_id);

    Ok(StoreResult {
        id: new_id,
        created: true,
        duplicate_of: None,
    })
}

/// Insert a memory row inside an existing SQLite transaction.
fn store_transactional_rusqlite(
    tx: &rusqlite::Transaction<'_>,
    content: &str,
    req: &StoreRequest,
    user_id: i64,
    importance: i32,
    tags_json: Option<String>,
    category: &str,
) -> Result<i64> {
    let (version, root_memory_id) = if let Some(parent_id) = req.parent_memory_id {
        let mut stmt = tx.prepare("SELECT version, root_memory_id FROM memories WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![parent_id])?;
        if let Some(parent_row) = rows.next()? {
            let parent_version: i32 = parent_row.get(0)?;
            let parent_root: Option<i64> = parent_row.get(1)?;
            let root = parent_root.unwrap_or(parent_id);
            (parent_version + 1, Some(root))
        } else {
            return Err(EngError::NotFound(format!(
                "parent memory {} not found",
                parent_id
            )));
        }
    } else {
        (1, None)
    };

    if let Some(parent_id) = req.parent_memory_id {
        // Only supersede a parent that is itself the current latest version.
        // Without the `is_latest = 1` guard, storing against an already
        // superseded parent would leave the real head untouched and the new
        // INSERT (is_latest = 1) would create a second live head, forking the
        // version chain. A zero-row update means the parent is stale -- refuse.
        let affected = tx
            .execute(
                "UPDATE memories SET is_latest = 0, updated_at = datetime('now') WHERE id = ?1 AND is_latest = 1 AND user_id = ?2",
                rusqlite::params![parent_id, user_id],
            )
            ?;
        if affected == 0 {
            return Err(EngError::Conflict(format!(
                "parent memory {} is not the latest version; refusing to fork the chain",
                parent_id
            )));
        }
    }

    let is_static = req.is_static.unwrap_or(false) as i32;
    tx.execute(
        "INSERT INTO memories (
            content, category, source, session_id, importance,
            version, is_latest, parent_memory_id, root_memory_id,
            is_static, tags, status, space_id,
            fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state,
            fsrs_reps, fsrs_lapses, model, sync_id, user_id
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, 1, ?7, ?8,
            ?9, ?10, 'approved', ?11,
            1.0, 1.0, 0,
            0, 0, ?12, ?13, ?14
        )",
        rusqlite::params![
            content,
            category,
            req.source.clone(),
            req.session_id.clone(),
            importance,
            version,
            req.parent_memory_id,
            root_memory_id,
            is_static,
            tags_json,
            req.space_id,
            Option::<String>::None,
            req.sync_id.clone(),
            user_id
        ],
    )?;

    let new_id = tx.last_insert_rowid();

    if let Some(ref emb) = req.embedding {
        let emb_blob = embedding_to_blob(emb);
        tx.execute(
            "UPDATE memories SET embedding_vec_1024 = ?1 WHERE id = ?2",
            rusqlite::params![emb_blob, new_id],
        )?;
    }

    Ok(new_id)
}

/// Retrieve a memory by ID for content access. Filters out forgotten and archived memories.
#[tracing::instrument(skip(db))]
pub async fn get(db: &Database, id: i64, user_id: i64) -> Result<Memory> {
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND is_forgotten = 0 AND is_archived = 0",
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
        "SELECT {} FROM memories WHERE id = ?1 AND is_forgotten = 0",
        MEMORY_COLUMNS
    );
    get_internal(db, id, user_id, &sql, false).await
}

/// Fetch a memory with an appended owner predicate and optional access logging.
async fn get_internal(
    db: &Database,
    id: i64,
    user_id: i64,
    sql: &str,
    log_access: bool,
) -> Result<Memory> {
    // The caller-supplied `sql` ends with the `WHERE id = ?1 AND ...` filters;
    // append the owner predicate (bound as ?2) so single-DB (shared) mode never
    // returns another user's memory by id. A no-op in a single-owner shard.
    let sql_for_read = format!("{sql} AND user_id = ?2");
    let memory = db
        .read(move |conn| {
            let mut stmt = conn.prepare(&sql_for_read)?;
            let mut rows = stmt.query(rusqlite::params![id, user_id])?;
            if let Some(row) = rows.next()? {
                row_to_memory(row, user_id)
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
            )?;
            Ok(())
        })
        .await?;
    }

    Ok(memory)
}

/// List active memories for a required owner with optional filters.
#[tracing::instrument(skip(db, opts), fields(user_id = opts.user_id.unwrap_or(0), limit = opts.limit))]
pub async fn list(db: &Database, opts: ListOptions) -> Result<Vec<Memory>> {
    // SECURITY (SEC-C3): user_id MUST be set. Without a tenant filter the
    // query returns every user's memories. All HTTP handlers set this, but
    // a missing guard here would silently expose all data if any future
    // internal caller uses ListOptions::default().
    let owner_user_id = opts.user_id.ok_or_else(|| {
        crate::EngError::InvalidInput("user_id is required for memory listing".into())
    })?;

    // Build WHERE clauses with parameterized values to prevent SQL injection
    let mut conditions = vec!["1=1".to_string()];
    let mut param_values: Vec<rusqlite::types::Value> = Vec::new();
    let mut param_idx = 1;

    // Always scope to the owner. Unconditional so single-DB (shared) mode is
    // isolated; a no-op in a single-owner shard.
    conditions.push(format!("user_id = ?{}", param_idx));
    param_values.push(rusqlite::types::Value::Integer(owner_user_id));
    param_idx += 1;

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
        let mut stmt = conn.prepare(&sql)?;
        let rusqlite_params = rusqlite::params_from_iter(param_values.iter().cloned());
        let mut rows = stmt.query(rusqlite_params)?;
        let mut memories = Vec::with_capacity(cap);
        while let Some(row) = rows.next()? {
            memories.push(row_to_memory(row, owner_user_id)?);
        }
        Ok(memories)
    })
    .await
}

/// Soft-delete an owned memory by marking it forgotten.
#[tracing::instrument(skip(db))]
pub async fn delete(db: &Database, id: i64, user_id: i64) -> Result<()> {
    // Soft delete -- set is_forgotten, record reason
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE memories SET \
                    is_forgotten = 1, \
                    forget_reason = 'user_deleted', \
                    updated_at = datetime('now') \
                 WHERE id = ?1 AND is_forgotten = 0 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )?)
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
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, 1).await {
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
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params![user_id, limit as i64])?;
        // 6.9 capacity hint: LIMIT bounds the row count.
        let mut result = Vec::with_capacity(limit);
        while let Some(row) = rows.next()? {
            result.push(row_to_memory(row, user_id)?);
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
            Ok(conn.execute(
                "UPDATE memories SET \
                    is_forgotten = 0, \
                    forget_reason = NULL, \
                    updated_at = datetime('now') \
                 WHERE id = ?1 AND is_forgotten = 1 AND forget_reason = 'user_deleted' AND user_id = ?2",
                rusqlite::params![id, user_id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "memory {} not found in trash",
            id
        )));
    }
    search::invalidate_search_cache(user_id);
    // Queue vector reinsert so restored memory appears in semantic search.
    // The vector was deleted on forget; the replay sweeper will re-embed it.
    record_vector_sync_failure(db, id, user_id, "insert", "restore-reinsert").await;
    get(db, id, user_id).await
}

/// Permanently delete memories that have been in the trash longer than the
/// retention window (default 30 days). Returns the number of purged rows.
///
/// When `update_counters` is true (tenant shard mode), reads the total
/// content size and count of rows to be deleted, then decrements
/// tenant_state counters in the same write closure.
#[tracing::instrument(skip(db))]
pub async fn purge_trashed(
    db: &Database,
    retention_days: i64,
    update_counters: bool,
) -> Result<usize> {
    db.write(move |conn| {
        let cutoff = format!("-{} days", retention_days);

        if update_counters {
            let (del_bytes, del_count): (i64, i64) = conn.query_row(
                "SELECT COALESCE(SUM(length(content)), 0), COUNT(*) \
                     FROM memories \
                     WHERE is_forgotten = 1 \
                       AND forget_reason = 'user_deleted' \
                       AND updated_at < datetime('now', ?1)",
                rusqlite::params![cutoff],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;

            let n = conn.execute(
                "DELETE FROM memories \
                     WHERE is_forgotten = 1 \
                       AND forget_reason = 'user_deleted' \
                       AND updated_at < datetime('now', ?1)",
                rusqlite::params![cutoff],
            )?;

            if del_bytes > 0 || del_count > 0 {
                conn.execute(
                    "UPDATE tenant_state SET value = MAX(0, value - ?1), \
                     updated_at = datetime('now') WHERE key = 'content_bytes'",
                    rusqlite::params![del_bytes],
                )?;
                conn.execute(
                    "UPDATE tenant_state SET value = MAX(0, value - ?1), \
                     updated_at = datetime('now') WHERE key = 'memory_count'",
                    rusqlite::params![del_count],
                )?;
            }
            Ok(n)
        } else {
            Ok(conn.execute(
                "DELETE FROM memories \
                 WHERE is_forgotten = 1 \
                   AND forget_reason = 'user_deleted' \
                   AND updated_at < datetime('now', ?1)",
                rusqlite::params![cutoff],
            )?)
        }
    })
    .await
}

/// Update an existing memory by id, creating a new versioned row.
///
/// When `update_counters` is true (tenant shard mode), applies the
/// content-bytes delta to the tenant_state counter inside the write
/// transaction to keep quota tracking consistent.
#[tracing::instrument(skip(db, req))]
pub async fn update(
    db: &Database,
    id: i64,
    mut req: UpdateRequest,
    user_id: i64,
    update_counters: bool,
) -> Result<Memory> {
    // SEC-recall-1.8: L2-normalize a supplied embedding so cosine semantics
    // hold regardless of provider. Mirrors the same step in `store`.
    if let Some(ref mut emb) = req.embedding {
        crate::embeddings::normalize::l2_normalize(emb);
    }

    // 1. Get the existing memory (outside transaction - read only). Scope by
    // owner so single-DB mode cannot update another user's memory by id.
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND is_forgotten = 0 AND user_id = ?2",
        MEMORY_COLUMNS
    );

    let sql_for_read = sql.clone();
    let old = db
        .read(move |conn| {
            let mut stmt = conn.prepare(&sql_for_read)?;
            let mut rows = stmt.query(rusqlite::params![id, user_id])?;
            if let Some(row) = rows.next()? {
                row_to_memory(row, user_id)
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

    // Capture old content length before the transaction for counter delta.
    let old_content_len = old.content.len() as i64;

    let old_for_tx = old.clone();
    let embedding_for_tx = req.embedding.clone();
    let new_content_for_tx = new_content.clone();
    let new_category_for_tx = new_category.clone();
    let new_status_for_tx = new_status.clone();
    let new_tags_json_for_tx = new_tags_json.clone();
    let old_content_len_for_tx = old_content_len;

    let new_id = db
        .transaction(move |tx| {
            let result = update_transactional_rusqlite(
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
            )?;

            // E2: update content_bytes counter with the delta.
            if update_counters {
                let delta_bytes = new_content_for_tx.len() as i64 - old_content_len_for_tx;
                if delta_bytes != 0 {
                    tx.execute(
                        "UPDATE tenant_state SET value = MAX(0, value + ?1), \
                         updated_at = datetime('now') WHERE key = 'content_bytes'",
                        rusqlite::params![delta_bytes],
                    )
                    .map_err(|e| {
                        crate::EngError::DatabaseMessage(format!("counter update failed: {e}"))
                    })?;
                }
            }

            Ok(result)
        })
        .await?;

    // SEC-recall-1.7: keep LanceDB in sync with the new version row.
    // Resolve the embedding to insert: either the freshly-supplied one, or
    // the blob we carried forward inside the transaction. If neither exists
    // (old row had NULL embedding), skip both insert and delete entirely.
    // The old version's LanceDB row is only deleted AFTER the new insert
    // succeeds -- otherwise a transient LanceDB failure would erase the
    // memory's only vector row, leaving it invisible to vector search.
    if let Some(index) = db.vector_index.as_ref() {
        let resolved_embedding: Option<Vec<f32>> = if let Some(ref emb) = req.embedding {
            Some(emb.clone())
        } else {
            let blob: Option<Vec<u8>> = db
                .read(move |conn| {
                    Ok(conn.query_row(
                        "SELECT embedding_vec_1024 FROM memories WHERE id = ?1",
                        rusqlite::params![new_id],
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )?)
                })
                .await?;
            blob.map(|b| {
                b.chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect()
            })
        };

        if let Some(emb) = resolved_embedding {
            match index.insert(new_id, &emb).await {
                Ok(()) => {
                    if let Err(e) = index.delete(id).await {
                        warn!(
                            "LanceDB vector delete failed for superseded memory {}: {}",
                            id, e
                        );
                        record_vector_sync_failure(db, id, user_id, "delete", &e.to_string()).await;
                    }
                }
                Err(e) => {
                    warn!("LanceDB vector insert failed for memory {}: {}", new_id, e);
                    record_vector_sync_failure(db, new_id, user_id, "insert", &e.to_string()).await;
                    // Insert failed -- intentionally do NOT delete the old
                    // row so the memory stays searchable via the previous
                    // version's vector entry until the sync replay catches up.
                }
            }
        }
    }

    // Carry forward or replace chunk embeddings for the new version.
    if let Some(ref chunks) = req.chunk_embeddings {
        write_chunks(db, new_id, chunks).await;
    } else {
        carry_forward_chunks(db, id, new_id).await;
    }

    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, 1).await {
        warn!("pagerank dirty mark failed on update: {}", e);
    }
    search::invalidate_search_cache(user_id);

    let new_sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2",
        MEMORY_COLUMNS
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&new_sql)?;
        let mut rows = stmt.query(rusqlite::params![new_id, user_id])?;
        if let Some(row) = rows.next()? {
            row_to_memory(row, user_id)
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
    let affected = tx.execute(
        "UPDATE memories SET is_latest = 0, updated_at = datetime('now') \
             WHERE id = ?1 AND is_latest = 1 AND user_id = ?2",
        rusqlite::params![old_id, user_id],
    )?;
    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "memory {} is no longer the latest version (concurrent update)",
            old_id
        )));
    }

    // The new version row must carry forward the previous version's lifecycle
    // and linkage state. Omitting these columns let SQLite apply table
    // defaults, silently wiping is_archived/is_fact/episode_id/forget_*/
    // valence/etc. on every content update. Carry them all forward from `old`.
    tx.execute(
        "INSERT INTO memories (
            content, category, source, session_id, importance,
            version, is_latest, parent_memory_id, root_memory_id,
            is_static, tags, status, space_id,
            fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength,
            fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at,
            confidence, model,
            is_archived, is_fact, is_decomposed, source_count,
            episode_id, forget_after, forget_reason, decay_score,
            sync_id, valence, arousal, dominant_emotion, user_id
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, 1, ?7, ?8,
            ?9, ?10, ?11, ?12,
            ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20,
            ?21, ?22,
            ?23, ?24, ?25, ?26,
            ?27, ?28, ?29, ?30,
            ?31, ?32, ?33, ?34, ?35
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
            old.model.clone(),
            old.is_archived,
            old.is_fact,
            old.is_decomposed,
            old.source_count,
            old.episode_id,
            old.forget_after.clone(),
            old.forget_reason.clone(),
            old.decay_score,
            old.sync_id.clone(),
            old.valence,
            old.arousal,
            old.dominant_emotion.clone(),
            user_id
        ],
    )?;

    let new_id = tx.last_insert_rowid();

    if let Some(emb) = embedding {
        let emb_blob = embedding_to_blob(emb);
        tx.execute(
            "UPDATE memories SET embedding_vec_1024 = ?1 WHERE id = ?2",
            rusqlite::params![emb_blob, new_id],
        )?;
    } else {
        // SEC-recall-1.7: when the caller does not supply a fresh embedding,
        // carry forward the old version's `embedding_vec_1024` blob so the
        // new version row stays vector-searchable. Without this, an update
        // that only changes content would leave the new version row with a
        // NULL embedding -- invisible to vector search until manually
        // re-embedded. NULL on the source row stays NULL on the target row,
        // which is what we want.
        tx.execute(
            "UPDATE memories SET embedding_vec_1024 = \
                 (SELECT embedding_vec_1024 FROM memories WHERE id = ?1) \
             WHERE id = ?2",
            rusqlite::params![old_id, new_id],
        )?;
    }

    Ok(new_id)
}

// -- Additional DB operations matching TS db.ts ---

#[tracing::instrument(skip(db))]
pub async fn mark_forgotten(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE memories SET is_forgotten = 1, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )?)
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
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, 1).await {
        warn!(
            "mark_pagerank_dirty failed after mark_forgotten for user {}: {}",
            user_id, e
        );
    }
    search::invalidate_search_cache(user_id);
    Ok(())
}

/// Mark an owned memory as archived so active retrieval excludes it.
#[tracing::instrument(skip(db))]
pub async fn mark_archived(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE memories SET is_archived = 1, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    }
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, 1).await {
        warn!(
            "mark_pagerank_dirty failed after mark_archived for user {}: {}",
            user_id, e
        );
    }
    search::invalidate_search_cache(user_id);
    Ok(())
}

/// Restore an owned archived memory to the active corpus.
#[tracing::instrument(skip(db))]
pub async fn mark_unarchived(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE memories SET is_archived = 0, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    }
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, 1).await {
        warn!(
            "mark_pagerank_dirty failed after mark_unarchived for user {}: {}",
            user_id, e
        );
    }
    search::invalidate_search_cache(user_id);
    Ok(())
}

/// Store the forget reason for an owned memory.
#[tracing::instrument(skip(db, reason))]
pub async fn update_forget_reason(
    db: &Database,
    id: i64,
    reason: &str,
    user_id: i64,
) -> Result<()> {
    let reason = reason.to_string();
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE memories SET forget_reason = ?1 WHERE id = ?2 AND user_id = ?3",
                rusqlite::params![reason, id, user_id],
            )?)
        })
        .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    }
    Ok(())
}

/// Adjust an owned memory's importance while keeping it in range.
#[tracing::instrument(skip(db))]
pub async fn adjust_importance(
    db: &Database,
    memory_id: i64,
    user_id: i64,
    delta: i32,
) -> Result<()> {
    let affected = db.write(move |conn| {
        let sql = if delta > 0 {
            "UPDATE memories SET importance = MIN(importance + ?1, 10) WHERE id = ?2 AND user_id = ?3"
        } else {
            "UPDATE memories SET importance = MAX(importance + ?1, 0) WHERE id = ?2 AND user_id = ?3"
        };
        Ok(conn.execute(sql, rusqlite::params![delta, memory_id, user_id])?)
    })
    .await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!(
            "memory {} not found",
            memory_id
        )));
    }
    Ok(())
}

/// Insert a link between two owned, active memories.
#[tracing::instrument(skip(db))]
pub async fn insert_link(
    db: &Database,
    source_id: i64,
    target_id: i64,
    similarity: f64,
    link_type: &str,
    user_id: i64,
) -> Result<()> {
    // Validate both memories exist and are not forgotten
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
            ?;
        if count < 2 {
            return Err(EngError::NotFound(format!(
                "one or both memories ({}, {}) not found",
                source_id, target_id
            )));
        }
        conn.execute(
            "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![source_id, target_id, similarity, link_type],
        )
        ?;
        Ok(())
    })
    .await?;
    if let Err(e) = crate::graph::pagerank::mark_pagerank_dirty(db, 1).await {
        warn!(
            "mark_pagerank_dirty failed after insert_link for user {}: {}",
            user_id, e
        );
    }
    search::invalidate_search_cache(user_id);
    Ok(())
}

/// Update the source-count metadata for an owned memory.
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
        )?;
        Ok(())
    })
    .await
}

/// List all normalized tags used by active memories for one owner.
#[tracing::instrument(skip(db))]
pub async fn list_all_tags(db: &Database, user_id: i64) -> Result<Vec<TagCount>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT tags FROM memories
                 WHERE user_id = ?1
                   AND is_forgotten = 0
                   AND is_latest = 1
                   AND tags IS NOT NULL
                   AND tags != '[]'",
        )?;
        let mut rows = stmt.query(rusqlite::params![user_id])?;

        let mut counts: HashMap<String, i64> = HashMap::new();
        while let Some(row) = rows.next()? {
            let raw_tags: Option<String> = row.get(0)?;
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

/// Search active owned memories by normalized tag membership.
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
    let placeholders: Vec<String> = (0..tag_count).map(|i| format!("?{}", i + 1)).collect();

    // The owner predicate binds at index tag_count + 1 (after the tag params).
    let user_param = tag_count + 1;
    let sql = if match_all {
        // match_all: memory must contain ALL requested tags.
        // Count distinct matches from json_each; must equal tag_count.
        format!(
            "SELECT {} FROM memories m
             WHERE m.user_id = ?{}
               AND m.is_forgotten = 0
               AND m.is_latest = 1
               AND m.tags IS NOT NULL
               AND (SELECT COUNT(DISTINCT je.value)
                    FROM json_each(m.tags) je
                    WHERE je.value IN ({})) = {}
             ORDER BY m.created_at DESC
             LIMIT {}",
            MEMORY_COLUMNS,
            user_param,
            placeholders.join(", "),
            tag_count,
            limit
        )
    } else {
        // match_any: memory must contain at least one requested tag.
        format!(
            "SELECT {} FROM memories m
             WHERE m.user_id = ?{}
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
            user_param,
            placeholders.join(", "),
            limit
        )
    };

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql)?;
        // Bind each tag at indices 1..N, then the owner id at N+1.
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> =
            Vec::with_capacity(tag_count + 1);
        for tag in &normalized {
            params_vec.push(Box::new(tag.clone()));
        }
        params_vec.push(Box::new(user_id));
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut rows = stmt.query(param_refs.as_slice())?;
        // 6.9 capacity hint: LIMIT bounds the row count.
        let mut memories = Vec::with_capacity(limit);
        while let Some(row) = rows.next()? {
            memories.push(row_to_memory(row, user_id)?);
        }
        Ok(memories)
    })
    .await
}

/// Replace the normalized tag list on an owned memory.
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
            "UPDATE memories SET tags = ?1, updated_at = datetime('now') WHERE id = ?2",
            rusqlite::params![normalized, memory_id],
        )?;
        Ok(())
    })
    .await?;
    search::invalidate_search_cache(user_id);
    Ok(())
}

/// Return non-forgotten links connected to an owned memory id.
#[tracing::instrument(skip(db))]
pub async fn get_links_for(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<LinkedMemory>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
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
        )?;
        let mut rows = stmt.query(rusqlite::params![memory_id, user_id])?;

        // 6.9 capacity hint: link fanout typically small.
        let mut links = Vec::with_capacity(16);
        while let Some(row) = rows.next()? {
            if row.get::<_, i32>(5)? != 0 {
                continue;
            }
            links.push(LinkedMemory {
                id: row.get(0)?,
                similarity: row.get(1)?,
                link_type: row.get(2)?,
                content: row.get(3)?,
                category: row.get(4)?,
            });
        }
        Ok(links)
    })
    .await
}

/// Return the version chain for an owned memory root.
#[tracing::instrument(skip(db))]
pub async fn get_version_chain(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<VersionChainEntry>> {
    let memory = get(db, memory_id, user_id).await?;
    let root_id = memory.root_memory_id.unwrap_or(memory.id);

    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, content, version, is_latest
                 FROM memories
                 WHERE (root_memory_id = ?1 OR id = ?1) AND user_id = ?2
                 ORDER BY version ASC",
        )?;
        let mut rows = stmt.query(rusqlite::params![root_id, user_id])?;

        // 6.9 capacity hint: version chains are usually short.
        let mut chain = Vec::with_capacity(8);
        while let Some(row) = rows.next()? {
            chain.push(VersionChainEntry {
                id: row.get(0)?,
                content: row.get(1)?,
                version: row.get(2)?,
                is_latest: row.get::<_, i32>(3)? != 0,
            });
        }
        Ok(chain)
    })
    .await
}

/// Summarize a user's memory corpus for profile generation.
#[tracing::instrument(skip(db))]
pub async fn get_user_profile(db: &Database, user_id: i64) -> Result<UserProfile> {
    let (memory_count, oldest_memory, newest_memory, avg_importance) = db
        .read(move |conn| {
            Ok(conn.query_row(
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
            )?)
        })
        .await?;

    let top_categories = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT category, COUNT(*)
                     FROM memories
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
                     GROUP BY category
                     ORDER BY COUNT(*) DESC, category ASC
                     LIMIT 10",
            )?;
            let mut rows = stmt.query(rusqlite::params![user_id])?;

            // 6.9 capacity hint: SQL caps at LIMIT 10.
            let mut top_categories = Vec::with_capacity(10);
            while let Some(row) = rows.next()? {
                top_categories.push(CategoryCount {
                    category: row.get(0)?,
                    count: row.get(1)?,
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

/// Compute detailed per-user memory statistics.
#[tracing::instrument(skip(db))]
pub async fn get_user_stats(db: &Database, user_id: i64) -> Result<UserStats> {
    // Scope counts to the owner so single-DB (shared) mode reports per-user
    // stats. conversations, episodes, and entities now carry user_id (re-added
    // by the single-DB repair). The skills count is scoped once skill_records
    // carries user_id on shards (skills repair step).
    let memories: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )?)
        })
        .await?;
    let archived: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_archived = 1 AND is_latest = 1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )?)
        })
        .await?;
    let conversations: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM conversations WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )?)
        })
        .await?;
    let episodes: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM episodes WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )?)
        })
        .await?;
    let entities: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM entities WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )?)
        })
        .await?;
    let skills: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM skill_records WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get(0),
            )?)
        })
        .await?;

    let categories = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT category, COUNT(*)
                     FROM memories
                     WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
                     GROUP BY category
                     ORDER BY COUNT(*) DESC, category ASC",
            )?;
            let mut rows = stmt.query(rusqlite::params![user_id])?;
            let mut categories = BTreeMap::new();
            while let Some(row) = rows.next()? {
                categories.insert(row.get::<_, String>(0)?, row.get::<_, i64>(1)?);
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

/// Unit tests for memory row mapping and valence persistence.
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
                "INSERT INTO memories (content, category, source, importance, confidence, \
                 created_at, updated_at, is_latest, is_forgotten, is_archived) \
                 VALUES ('col-audit', 'general', 'test', 5, 1.0, \
                 datetime('now'), datetime('now'), 1, 0, 0)",
                params![],
            )?;
            Ok(())
        })
        .await
        .expect("seed");

        let sql = format!("SELECT {MEMORY_COLUMNS} FROM memories LIMIT 1");
        let got = db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql)?;
                let mem = stmt.query_row(params![], |row| {
                    row_to_memory(row, 1).map_err(|e| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                            e.to_string(),
                        )))
                    })
                })?;
                Ok(mem)
            })
            .await
            .expect("select");
        assert_eq!(got.content, "col-audit");
    }

    /// Build a minimal store request for valence tests.
    fn valence_store_request(content: &str, user_id: i64) -> crate::memory::types::StoreRequest {
        crate::memory::types::StoreRequest {
            content: content.to_string(),
            category: "test".to_string(),
            source: "test".to_string(),
            user_id: Some(user_id),
            ..Default::default()
        }
    }

    /// Read persisted valence fields for a memory.
    async fn read_valence(db: &Database, id: i64) -> (Option<f64>, Option<String>) {
        db.read(move |conn| {
            Ok(conn.query_row(
                "SELECT valence, dominant_emotion FROM memories WHERE id = ?1",
                rusqlite::params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .await
        .expect("read valence")
    }

    /// Positive affective content should persist positive valence metadata.
    #[tokio::test]
    async fn store_persists_positive_valence_for_happy_content() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let user_id = 1;
        let stored = store(
            &db,
            valence_store_request("I am ecstatic and thrilled about this release", user_id),
            None,
            false,
        )
        .await
        .expect("store");
        let (valence, emotion) = read_valence(&db, stored.id).await;
        assert!(valence.unwrap_or(0.0) > 0.5, "valence should be positive");
        assert_ne!(emotion.unwrap_or_default(), "");
    }

    /// Negative affective content should persist negative valence metadata.
    #[tokio::test]
    async fn store_persists_negative_valence_for_angry_content() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let user_id = 1;
        let stored = store(
            &db,
            valence_store_request("I am furious and everything crashed", user_id),
            None,
            false,
        )
        .await
        .expect("store");
        let (valence, emotion) = read_valence(&db, stored.id).await;
        assert!(valence.unwrap_or(0.0) < 0.0, "valence should be negative");
        assert_ne!(emotion.unwrap_or_default(), "");
    }

    /// Neutral factual content should not force valence metadata.
    #[tokio::test]
    async fn store_leaves_valence_null_for_neutral_content() {
        let db = Database::connect_memory().await.expect("in-mem db");
        let user_id = 1;
        let stored = store(
            &db,
            valence_store_request("The meeting is at 3pm tomorrow", user_id),
            None,
            false,
        )
        .await
        .expect("store");
        let (valence, emotion) = read_valence(&db, stored.id).await;
        assert_eq!(valence, None, "neutral content leaves valence null");
        assert_eq!(emotion, None);
    }
}
