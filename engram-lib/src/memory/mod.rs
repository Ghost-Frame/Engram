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
use libsql::params;
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

fn embedding_to_json(embedding: &[f32]) -> String {
    format!(
        "[{}]",
        embedding
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// Map a libsql Row to a Memory struct.
/// Column order must match the SELECT in query_memories and related queries.
/// Order: id, content, category, source, session_id, importance, version,
///   is_latest, parent_memory_id, root_memory_id, source_count, is_static,
///   is_forgotten, is_archived, is_inference, is_fact, is_decomposed,
///   forget_after, forget_reason, model, recall_hits, recall_misses,
///   adaptive_score, pagerank_score, last_accessed_at, access_count, tags,
///   episode_id, decay_score, confidence, sync_id, status, user_id, space_id,
///   fsrs_stability, fsrs_difficulty, fsrs_storage_strength,
///   fsrs_retrieval_strength, fsrs_learning_state, fsrs_reps, fsrs_lapses,
///   fsrs_last_review_at, valence, arousal, dominant_emotion,
///   created_at, updated_at
pub(crate) fn row_to_memory(row: &libsql::Row) -> Result<Memory> {
    Ok(Memory {
        id: row.get::<i64>(0)?,
        content: row.get::<String>(1)?,
        category: row.get::<String>(2)?,
        source: row.get::<String>(3)?,
        session_id: row.get::<Option<String>>(4)?,
        importance: row.get::<i32>(5)?,
        embedding: None, // embedding BLOB not fetched in standard queries
        version: row.get::<i32>(6)?,
        is_latest: row.get::<i32>(7)? != 0,
        parent_memory_id: row.get::<Option<i64>>(8)?,
        root_memory_id: row.get::<Option<i64>>(9)?,
        source_count: row.get::<i32>(10)?,
        is_static: row.get::<i32>(11)? != 0,
        is_forgotten: row.get::<i32>(12)? != 0,
        is_archived: row.get::<i32>(13)? != 0,
        is_inference: row.get::<i32>(14)? != 0,
        is_fact: row.get::<i32>(15)? != 0,
        is_decomposed: row.get::<i32>(16)? != 0,
        forget_after: row.get::<Option<String>>(17)?,
        forget_reason: row.get::<Option<String>>(18)?,
        model: row.get::<Option<String>>(19)?,
        recall_hits: row.get::<i32>(20)?,
        recall_misses: row.get::<i32>(21)?,
        adaptive_score: row.get::<Option<f64>>(22)?,
        pagerank_score: row.get::<Option<f64>>(23)?,
        last_accessed_at: row.get::<Option<String>>(24)?,
        access_count: row.get::<i32>(25)?,
        tags: row.get::<Option<String>>(26)?,
        episode_id: row.get::<Option<i64>>(27)?,
        decay_score: row.get::<Option<f64>>(28)?,
        confidence: row.get::<f64>(29)?,
        sync_id: row.get::<Option<String>>(30)?,
        status: row.get::<String>(31)?,
        user_id: row.get::<i64>(32)?,
        space_id: row.get::<Option<i64>>(33)?,
        fsrs_stability: row.get::<Option<f64>>(34)?,
        fsrs_difficulty: row.get::<Option<f64>>(35)?,
        fsrs_storage_strength: row.get::<Option<f64>>(36)?,
        fsrs_retrieval_strength: row.get::<Option<f64>>(37)?,
        fsrs_learning_state: row.get::<Option<i32>>(38)?,
        fsrs_reps: row.get::<Option<i32>>(39)?,
        fsrs_lapses: row.get::<Option<i32>>(40)?,
        fsrs_last_review_at: row.get::<Option<String>>(41)?,
        valence: row.get::<Option<f64>>(42)?,
        arousal: row.get::<Option<f64>>(43)?,
        dominant_emotion: row.get::<Option<String>>(44)?,
        created_at: row.get::<String>(45)?,
        updated_at: row.get::<String>(46)?,
        is_superseded: row.get::<i32>(47).map(|v| v != 0)?,
    })
}

/// Standard SELECT column list -- matches row_to_memory index order.
pub(crate) const MEMORY_COLUMNS: &str = "id, content, category, source, session_id, importance, \
    version, is_latest, parent_memory_id, root_memory_id, source_count, is_static, \
    is_forgotten, is_archived, is_inference, is_fact, is_decomposed, \
    forget_after, forget_reason, model, recall_hits, recall_misses, \
    adaptive_score, pagerank_score, last_accessed_at, access_count, tags, \
    episode_id, decay_score, confidence, sync_id, status, user_id, space_id, \
    fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength, \
    fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at, \
    valence, arousal, dominant_emotion, created_at, updated_at, is_superseded";

// -- Public CRUD functions ---

pub async fn store(db: &Database, req: StoreRequest) -> Result<StoreResult> {
    // 1. Validate content
    let content = req.content.trim().to_string();
    if content.is_empty() {
        return Err(EngError::InvalidInput("content cannot be empty".to_string()));
    }
    if content.len() > MAX_CONTENT_SIZE {
        return Err(EngError::InvalidInput(format!(
            "content exceeds maximum size of {} bytes",
            MAX_CONTENT_SIZE
        )));
    }

    let user_id = req.user_id.unwrap_or(1);
    let importance = clamp_importance(req.importance);

    // 2. Compute simhash of content
    let content_hash = simhash::simhash(&content);

    // 3. Check for duplicates -- query last 1000 memories for this user
    let dup_sql = "SELECT id, content FROM memories \
        WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1 \
        ORDER BY id DESC LIMIT 1000";
    let mut dup_rows = db.conn.query(dup_sql, params![user_id]).await?;
    while let Some(row) = dup_rows.next().await? {
        let existing_id: i64 = row.get(0)?;
        let existing_content: String = row.get(1)?;
        let existing_hash = simhash::simhash(&existing_content);
        if simhash::hamming_distance(content_hash, existing_hash) < 3 {
            return Ok(StoreResult {
                id: existing_id,
                created: false,
                duplicate_of: Some(existing_id),
            });
        }
    }

    // 4. Normalize tags
    let tags_json = normalize_tags(&req.tags);

    // 5. Determine versioning fields if parent_memory_id is set
    let (version, root_memory_id) = if let Some(parent_id) = req.parent_memory_id {
        // Fetch parent to get its version and root
        let parent_sql = "SELECT version, root_memory_id FROM memories WHERE id = ?1".to_string();
        let mut parent_rows = db.conn.query(&parent_sql, params![parent_id]).await?;
        if let Some(parent_row) = parent_rows.next().await? {
            let parent_version: i32 = parent_row.get(0)?;
            let parent_root: Option<i64> = parent_row.get(1)?;
            // root is either the parent's root or the parent itself if it has none
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

    // 6. Mark parent as not latest if versioning
    if let Some(parent_id) = req.parent_memory_id {
        db.conn
            .execute(
                "UPDATE memories SET is_latest = 0, updated_at = datetime('now') WHERE id = ?1",
                params![parent_id],
            )
            .await?;
    }

    let is_static = req.is_static.unwrap_or(false) as i32;

    // 7. INSERT the new memory
    let insert_sql = "INSERT INTO memories (
        content, category, source, session_id, importance,
        version, is_latest, parent_memory_id, root_memory_id,
        is_static, tags, status, user_id, space_id,
        fsrs_storage_strength, fsrs_retrieval_strength, fsrs_learning_state,
        fsrs_reps, fsrs_lapses
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5,
        ?6, 1, ?7, ?8,
        ?9, ?10, 'approved', ?11, ?12,
        1.0, 1.0, 0,
        0, 0
    )";

    db.conn
        .execute(
            insert_sql,
            params![
                content,
                req.category,
                req.source,
                req.session_id,
                importance,
                version,
                req.parent_memory_id,
                root_memory_id,
                is_static,
                tags_json,
                user_id,
                req.space_id
            ],
        )
        .await?;

    // 8. Get the inserted row id
    let mut id_rows = db
        .conn
        .query("SELECT last_insert_rowid()", ())
        .await?;
    let new_id: i64 = if let Some(row) = id_rows.next().await? {
        row.get(0)?
    } else {
        return Err(EngError::Internal("failed to get last insert id".to_string()));
    };

    // 9. If embedding provided, UPDATE embedding_vec_1024 with JSON array
    if let Some(ref emb) = req.embedding {
        let emb_json = embedding_to_json(emb);
        db.conn
            .execute(
                "UPDATE memories SET embedding_vec_1024 = vector(?1) WHERE id = ?2",
                params![emb_json, new_id],
            )
            .await?;

        if let Some(index) = db.vector_index.as_ref() {
            if let Err(e) = index.insert(new_id, user_id, emb).await {
                warn!("LanceDB vector insert failed for memory {}: {}", new_id, e);
            }
        }
    }

    Ok(StoreResult {
        id: new_id,
        created: true,
        duplicate_of: None,
    })
}

pub async fn get(db: &Database, id: i64, user_id: i64) -> Result<Memory> {
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0",
        MEMORY_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![id, user_id]).await?;
    let memory = if let Some(row) = rows.next().await? {
        row_to_memory(&row)?
    } else {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    };

    // Log the access -- update access_count and last_accessed_at
    db.conn
        .execute(
            "UPDATE memories SET \
                access_count = access_count + 1, \
                last_accessed_at = datetime('now'), \
                updated_at = datetime('now') \
             WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .await?;

    Ok(memory)
}

pub async fn list(db: &Database, opts: ListOptions) -> Result<Vec<Memory>> {
    // Build WHERE clauses with parameterized values to prevent SQL injection
    let mut conditions = vec!["1=1".to_string()];
    let mut params: Vec<libsql::Value> = Vec::new();
    let mut param_idx = 1;

    if !opts.include_forgotten {
        conditions.push("is_forgotten = 0".to_string());
    }
    if !opts.include_archived {
        conditions.push("is_archived = 0".to_string());
    }
    // Always filter to latest version
    conditions.push("is_latest = 1".to_string());

    if let Some(ref cat) = opts.category {
        conditions.push(format!("category = ?{}", param_idx));
        params.push(libsql::Value::Text(cat.clone()));
        param_idx += 1;
    }
    if let Some(ref src) = opts.source {
        conditions.push(format!("source = ?{}", param_idx));
        params.push(libsql::Value::Text(src.clone()));
        param_idx += 1;
    }
    if let Some(uid) = opts.user_id {
        conditions.push(format!("user_id = ?{}", param_idx));
        params.push(libsql::Value::Integer(uid));
        param_idx += 1;
    }
    if let Some(sid) = opts.space_id {
        conditions.push(format!("space_id = ?{}", param_idx));
        params.push(libsql::Value::Integer(sid));
        param_idx += 1;
    }

    // Add limit and offset as parameters
    conditions.push(format!("1=1")); // placeholder for LIMIT/OFFSET which go after WHERE
    let where_clause = conditions.join(" AND ");

    let sql = format!(
        "SELECT {} FROM memories WHERE {} ORDER BY id DESC LIMIT ?{} OFFSET ?{}",
        MEMORY_COLUMNS, where_clause, param_idx, param_idx + 1
    );
    params.push(libsql::Value::Integer(opts.limit as i64));
    params.push(libsql::Value::Integer(opts.offset as i64));

    let mut rows = db.conn.query(&sql, params).await?;
    let mut memories = Vec::new();
    while let Some(row) = rows.next().await? {
        memories.push(row_to_memory(&row)?);
    }
    Ok(memories)
}

pub async fn delete(db: &Database, id: i64, user_id: i64) -> Result<()> {
    // Soft delete -- set is_forgotten, record reason
    let affected = db
        .conn
        .execute(
            "UPDATE memories SET \
                is_forgotten = 1, \
                forget_reason = 'user_deleted', \
                updated_at = datetime('now') \
             WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0",
            params![id, user_id],
        )
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
        }
    }
    Ok(())
}

pub async fn update(db: &Database, id: i64, req: UpdateRequest, user_id: i64) -> Result<Memory> {
    // 1. Get the existing memory, scoped to user_id
    let sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2 AND is_forgotten = 0",
        MEMORY_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![id, user_id]).await?;
    let old = if let Some(row) = rows.next().await? {
        row_to_memory(&row)?
    } else {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    };

    // 2. Mark old as not latest
    db.conn
        .execute(
            "UPDATE memories SET is_latest = 0, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
        )
        .await?;

    // 3. Compute new field values (fallback to old values)
    let new_content = req
        .content
        .as_deref()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| old.content.clone());

    if new_content.is_empty() {
        return Err(EngError::InvalidInput("content cannot be empty".to_string()));
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

    // Tags: if provided in request, normalize; otherwise keep old tags string
    let new_tags_json = if req.tags.is_some() {
        normalize_tags(&req.tags)
    } else {
        old.tags.clone()
    };

    // Determine root: old root_memory_id if set, else old.id is the root
    let new_root_memory_id = old.root_memory_id.unwrap_or(old.id);
    let new_version = old.version + 1;

    // 4. INSERT new row
    let insert_sql = "INSERT INTO memories (
        content, category, source, session_id, importance,
        version, is_latest, parent_memory_id, root_memory_id,
        is_static, tags, status, user_id, space_id,
        fsrs_stability, fsrs_difficulty, fsrs_storage_strength, fsrs_retrieval_strength,
        fsrs_learning_state, fsrs_reps, fsrs_lapses, fsrs_last_review_at,
        confidence
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5,
        ?6, 1, ?7, ?8,
        ?9, ?10, ?11, ?12, ?13,
        ?14, ?15, ?16, ?17,
        ?18, ?19, ?20, ?21,
        ?22
    )";

    db.conn
        .execute(
            insert_sql,
            params![
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
                old.confidence
            ],
        )
        .await?;

    // 5. Get new row id
    let mut id_rows = db.conn.query("SELECT last_insert_rowid()", ()).await?;
    let new_id: i64 = if let Some(row) = id_rows.next().await? {
        row.get(0)?
    } else {
        return Err(EngError::Internal(
            "failed to get last insert id after update".to_string(),
        ));
    };

    // 6. If embedding provided, UPDATE the vector column
    if let Some(ref emb) = req.embedding {
        let emb_json = embedding_to_json(emb);
        db.conn
            .execute(
                "UPDATE memories SET embedding_vec_1024 = vector(?1) WHERE id = ?2",
                params![emb_json, new_id],
            )
            .await?;

        if let Some(index) = db.vector_index.as_ref() {
            if let Err(e) = index.insert(new_id, user_id, emb).await {
                warn!("LanceDB vector insert failed for memory {}: {}", new_id, e);
            }
            if let Err(e) = index.delete(id).await {
                warn!("LanceDB vector delete failed for superseded memory {}: {}", id, e);
            }
        }
    }

    // 7. Fetch and return the new row
    let new_sql = format!(
        "SELECT {} FROM memories WHERE id = ?1 AND user_id = ?2",
        MEMORY_COLUMNS
    );
    let mut new_rows = db.conn.query(&new_sql, params![new_id, user_id]).await?;
    if let Some(row) = new_rows.next().await? {
        row_to_memory(&row)
    } else {
        Err(EngError::Internal(
            "failed to fetch newly created memory version".to_string(),
        ))
    }
}

// -- Additional DB operations matching TS db.ts ---

pub async fn mark_forgotten(db: &Database, id: i64, user_id: i64) -> Result<()> {
    let affected = db.conn.execute(
        "UPDATE memories SET is_forgotten = 1, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    ).await?;
    if affected == 0 {
        return Err(EngError::NotFound(format!("memory {} not found", id)));
    }
    if let Some(index) = db.vector_index.as_ref() {
        if let Err(e) = index.delete(id).await {
            warn!("LanceDB vector delete failed for forgotten memory {}: {}", id, e);
        }
    }
    Ok(())
}

pub async fn mark_archived(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn.execute(
        "UPDATE memories SET is_archived = 1, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    ).await?;
    Ok(())
}

pub async fn mark_unarchived(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.conn.execute(
        "UPDATE memories SET is_archived = 0, updated_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    ).await?;
    Ok(())
}

pub async fn update_forget_reason(db: &Database, id: i64, reason: &str, user_id: i64) -> Result<()> {
    db.conn.execute(
        "UPDATE memories SET forget_reason = ?1 WHERE id = ?2 AND user_id = ?3",
        params![reason.to_string(), id, user_id],
    ).await?;
    Ok(())
}

pub async fn adjust_importance(db: &Database, memory_id: i64, user_id: i64, delta: i32) -> Result<()> {
    if delta > 0 {
        db.conn.execute(
            "UPDATE memories SET importance = MIN(importance + ?1, 10) WHERE id = ?2 AND user_id = ?3",
            params![delta, memory_id, user_id],
        ).await?;
    } else {
        db.conn.execute(
            "UPDATE memories SET importance = MAX(importance + ?1, 0) WHERE id = ?2 AND user_id = ?3",
            params![delta, memory_id, user_id],
        ).await?;
    }
    Ok(())
}

pub async fn insert_link(db: &Database, source_id: i64, target_id: i64, similarity: f64, link_type: &str, user_id: i64) -> Result<()> {
    // Validate both memories belong to this user before inserting the link
    let count_sql = "SELECT COUNT(*) FROM memories WHERE id IN (?1, ?2) AND user_id = ?3 AND is_forgotten = 0";
    let mut rows = db.conn.query(count_sql, params![source_id, target_id, user_id]).await?;
    if let Some(row) = rows.next().await? {
        let count: i64 = row.get(0)?;
        if count < 2 {
            return Err(EngError::NotFound(
                format!("one or both memories ({}, {}) do not belong to user {}", source_id, target_id, user_id)
            ));
        }
    }
    db.conn.execute(
        "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type) VALUES (?1, ?2, ?3, ?4)",
        params![source_id, target_id, similarity, link_type.to_string()],
    ).await?;
    Ok(())
}

pub async fn update_source_count(db: &Database, id: i64, source_count: i32, user_id: i64) -> Result<()> {
    db.conn.execute(
        "UPDATE memories SET source_count = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
        params![source_count, id, user_id],
    ).await?;
    Ok(())
}

pub async fn build_lance_index_from_existing(db: &Database) -> Result<usize> {
    let Some(index) = db.vector_index.as_ref() else {
        return Ok(0);
    };

    let mut rows = db
        .conn
        .query(
            "SELECT id, user_id, vector_extract(embedding_vec_1024)
             FROM memories
             WHERE embedding_vec_1024 IS NOT NULL
               AND is_forgotten = 0
               AND is_latest = 1",
            (),
        )
        .await?;

    let mut count = 0usize;
    while let Some(row) = rows.next().await? {
        let memory_id: i64 = row.get(0)?;
        let user_id: i64 = row.get(1)?;
        let embedding_json: String = row.get(2)?;
        let embedding: Vec<f32> = serde_json::from_str(&embedding_json)?;
        index.insert(memory_id, user_id, &embedding).await?;
        count += 1;
        if count.is_multiple_of(1000) {
            tracing::info!(count, "rebuilt LanceDB vector index rows");
        }
    }

    Ok(count)
}

pub async fn list_all_tags(db: &Database, user_id: i64) -> Result<Vec<TagCount>> {
    let mut rows = db
        .conn
        .query(
            "SELECT tags FROM memories
             WHERE user_id = ?1
               AND is_forgotten = 0
               AND is_latest = 1
               AND tags IS NOT NULL
               AND tags != '[]'",
            params![user_id],
        )
        .await?;

    let mut counts: HashMap<String, i64> = HashMap::new();
    while let Some(row) = rows.next().await? {
        let raw_tags = row.get::<Option<String>>(0)?;
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

    let wanted: HashSet<&str> = normalized.iter().map(String::as_str).collect();
    let sql = format!(
        "SELECT {} FROM memories
         WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
         ORDER BY created_at DESC",
        MEMORY_COLUMNS
    );
    let mut rows = db.conn.query(&sql, params![user_id]).await?;
    let mut memories = Vec::new();

    while let Some(row) = rows.next().await? {
        let memory = row_to_memory(&row)?;
        let memory_tags: HashSet<String> = parse_tags_json(&memory.tags).into_iter().collect();
        let matched = if match_all {
            wanted.iter().all(|tag| memory_tags.contains(*tag))
        } else {
            wanted.iter().any(|tag| memory_tags.contains(*tag))
        };

        if matched {
            memories.push(memory);
            if memories.len() >= limit {
                break;
            }
        }
    }

    Ok(memories)
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

    db.conn
        .execute(
            "UPDATE memories SET tags = ?1, updated_at = datetime('now') WHERE id = ?2 AND user_id = ?3",
            params![normalized, memory_id, user_id],
        )
        .await?;
    Ok(())
}

pub async fn get_links_for(db: &Database, memory_id: i64, user_id: i64) -> Result<Vec<LinkedMemory>> {
    let mut rows = db
        .conn
        .query(
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
            params![memory_id, user_id],
        )
        .await?;

    let mut links = Vec::new();
    while let Some(row) = rows.next().await? {
        if row.get::<i32>(5).unwrap_or(0) != 0 {
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
}

pub async fn get_version_chain(
    db: &Database,
    memory_id: i64,
    user_id: i64,
) -> Result<Vec<VersionChainEntry>> {
    let memory = get(db, memory_id, user_id).await?;
    let root_id = memory.root_memory_id.unwrap_or(memory.id);

    let mut rows = db
        .conn
        .query(
            "SELECT id, content, version, is_latest
             FROM memories
             WHERE (root_memory_id = ?1 OR id = ?1)
               AND user_id = ?2
             ORDER BY version ASC",
            params![root_id, user_id],
        )
        .await?;

    let mut chain = Vec::new();
    while let Some(row) = rows.next().await? {
        chain.push(VersionChainEntry {
            id: row.get(0)?,
            content: row.get(1)?,
            version: row.get(2)?,
            is_latest: row.get::<i32>(3)? != 0,
        });
    }
    Ok(chain)
}

async fn count_user_rows(db: &Database, sql: &str, user_id: i64) -> Result<i64> {
    let mut rows = db.conn.query(sql, params![user_id]).await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0).unwrap_or(0)),
        None => Ok(0),
    }
}

pub async fn get_user_profile(db: &Database, user_id: i64) -> Result<UserProfile> {
    let mut rows = db
        .conn
        .query(
            "SELECT COUNT(*), MIN(created_at), MAX(created_at), AVG(importance)
             FROM memories
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
            params![user_id],
        )
        .await?;

    let (memory_count, oldest_memory, newest_memory, avg_importance) = match rows.next().await? {
        Some(row) => (
            row.get::<i64>(0).unwrap_or(0),
            row.get::<Option<String>>(1).unwrap_or(None),
            row.get::<Option<String>>(2).unwrap_or(None),
            row.get::<Option<f64>>(3).unwrap_or(None).unwrap_or(0.0),
        ),
        None => (0, None, None, 0.0),
    };

    let mut category_rows = db
        .conn
        .query(
            "SELECT category, COUNT(*)
             FROM memories
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
             GROUP BY category
             ORDER BY COUNT(*) DESC, category ASC
             LIMIT 10",
            params![user_id],
        )
        .await?;

    let mut top_categories = Vec::new();
    while let Some(row) = category_rows.next().await? {
        top_categories.push(CategoryCount {
            category: row.get(0)?,
            count: row.get(1)?,
        });
    }

    let top_tags = list_all_tags(db, user_id).await?.into_iter().take(10).collect();
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
    let conversations =
        count_user_rows(db, "SELECT COUNT(*) FROM conversations WHERE user_id = ?1", user_id)
            .await?;
    let episodes = count_user_rows(db, "SELECT COUNT(*) FROM episodes WHERE user_id = ?1", user_id)
        .await?;
    let entities = count_user_rows(db, "SELECT COUNT(*) FROM entities WHERE user_id = ?1", user_id)
        .await?;
    let skills = count_user_rows(
        db,
        "SELECT COUNT(*) FROM skill_records WHERE user_id = ?1",
        user_id,
    )
    .await?;

    let mut category_rows = db
        .conn
        .query(
            "SELECT category, COUNT(*)
             FROM memories
             WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1
             GROUP BY category
             ORDER BY COUNT(*) DESC, category ASC",
            params![user_id],
        )
        .await?;
    let mut categories = BTreeMap::new();
    while let Some(row) = category_rows.next().await? {
        categories.insert(row.get::<String>(0)?, row.get::<i64>(1)?);
    }

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
