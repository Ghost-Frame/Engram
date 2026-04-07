use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use engram_lib::memory::{
    self,
    search::hybrid_search,
    types::{ListOptions, SearchRequest, StoreRequest, UpdateRequest},
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/store", post(store_memory))
        .route("/memory", post(store_memory))
        .route("/memories", post(store_memory))
        .route("/search", post(search_memories))
        .route("/memories/search", post(search_memories))
        .route("/recall", post(recall))
        .route("/list", get(list_memories))
        .route("/memory/{id}", get(get_memory).delete(delete_memory))
        .route("/memory/{id}/update", post(update_memory))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a tags JSON string (e.g. `'["a","b"]'`) into a `Vec<String>`.
/// Returns an empty vec on failure or if None.
fn parse_tags(tags: &Option<String>) -> Vec<String> {
    tags.as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

/// Convert a Memory to a JSON Value suitable for API responses.
/// Tags are parsed from JSON string to array.
fn memory_to_json(m: &engram_lib::memory::types::Memory) -> Value {
    json!({
        "id": m.id,
        "content": m.content,
        "category": m.category,
        "source": m.source,
        "session_id": m.session_id,
        "importance": m.importance,
        "version": m.version,
        "is_latest": m.is_latest,
        "parent_memory_id": m.parent_memory_id,
        "root_memory_id": m.root_memory_id,
        "source_count": m.source_count,
        "is_static": m.is_static,
        "is_forgotten": m.is_forgotten,
        "is_archived": m.is_archived,
        "is_inference": m.is_inference,
        "is_fact": m.is_fact,
        "is_decomposed": m.is_decomposed,
        "forget_after": m.forget_after,
        "forget_reason": m.forget_reason,
        "model": m.model,
        "recall_hits": m.recall_hits,
        "recall_misses": m.recall_misses,
        "adaptive_score": m.adaptive_score,
        "pagerank_score": m.pagerank_score,
        "last_accessed_at": m.last_accessed_at,
        "access_count": m.access_count,
        "tags": parse_tags(&m.tags),
        "episode_id": m.episode_id,
        "decay_score": m.decay_score,
        "confidence": m.confidence,
        "sync_id": m.sync_id,
        "status": m.status,
        "user_id": m.user_id,
        "space_id": m.space_id,
        "created_at": m.created_at,
        "updated_at": m.updated_at,
    })
}

// ---------------------------------------------------------------------------
// POST /store  (also /memory, /memories)
// ---------------------------------------------------------------------------

async fn store_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<StoreRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Inject the authenticated user's ID
    req.user_id = Some(auth.user_id);

    // Compute embedding if provider available and none provided
    if req.embedding.is_none() {
        if let Some(ref embedder) = state.embedder {
            match embedder.embed(&req.content).await {
                Ok(emb) => req.embedding = Some(emb),
                Err(e) => tracing::warn!("embedding failed for store: {}", e),
            }
        }
    }

    let embedded = req.embedding.is_some();
    let result = memory::store(&state.db, req).await?;

    if let Some(existing_id) = result.duplicate_of {
        // Duplicate detected
        let body = json!({
            "stored": false,
            "duplicate": true,
            "existing_id": existing_id,
            "boosted": true,
        });
        return Ok((StatusCode::OK, Json(body)));
    }

    // New memory stored -- fetch the created memory to get decay_score etc.
    let mem = memory::get(&state.db, result.id).await?;
    let body = json!({
        "stored": true,
        "id": result.id,
        "created_at": mem.created_at,
        "importance": mem.importance,
        "embedded": embedded,
        "tags": parse_tags(&mem.tags),
        "decay_score": mem.decay_score.unwrap_or(mem.importance as f64),
    });

    Ok((StatusCode::CREATED, Json(body)))
}

// ---------------------------------------------------------------------------
// POST /search  (also /memories/search)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SearchBody {
    pub query: String,
    pub limit: Option<usize>,
    pub category: Option<String>,
    pub source: Option<String>,
    pub tags: Option<Vec<String>>,
    pub threshold: Option<f32>,
    pub space_id: Option<i64>,
    pub include_forgotten: Option<bool>,
}

async fn search_memories(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SearchBody>,
) -> Result<Json<Value>, AppError> {
    // Compute query embedding
    let embedding = if let Some(ref embedder) = state.embedder {
        match embedder.embed(&body.query).await {
            Ok(emb) => Some(emb),
            Err(e) => {
                tracing::warn!("embedding failed for search: {}", e);
                None
            }
        }
    } else {
        None
    };

    let req = SearchRequest {
        query: body.query,
        embedding,
        limit: body.limit,
        category: body.category,
        source: body.source,
        tags: body.tags,
        threshold: body.threshold,
        user_id: Some(auth.user_id),
        space_id: body.space_id,
        include_forgotten: body.include_forgotten,
    };

    let results = hybrid_search(&state.db, req).await?;

    let top_score = results.first().map(|r| r.score).unwrap_or(0.0);
    let abstained = results.is_empty();

    let result_items: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "id": r.memory.id,
                "content": r.memory.content,
                "category": r.memory.category,
                "source": r.memory.source,
                "importance": r.memory.importance,
                "created_at": r.memory.created_at,
                "score": r.score,
                "tags": parse_tags(&r.memory.tags),
                "search_type": r.search_type,
            })
        })
        .collect();

    Ok(Json(json!({
        "results": result_items,
        "abstained": abstained,
        "top_score": top_score,
    })))
}

// ---------------------------------------------------------------------------
// POST /recall
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RecallBody {
    pub query: String,
    pub limit: Option<usize>,
    pub space_id: Option<i64>,
}

async fn recall(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20);
    let user_id = auth.user_id;

    // Bucket 1: Static memories
    let static_opts = ListOptions {
        limit: 10,
        offset: 0,
        category: None,
        source: None,
        user_id: Some(user_id),
        space_id: body.space_id,
        include_forgotten: false,
        include_archived: false,
    };
    // We list and filter by is_static below since ListOptions has no is_static field.
    let all_list = memory::list(&state.db, static_opts).await?;
    let static_memories: Vec<_> = all_list.into_iter().filter(|m| m.is_static).collect();

    // Bucket 2: Semantic search via hybrid (vector + FTS)
    let query_embedding = if let Some(ref embedder) = state.embedder {
        match embedder.embed(&body.query).await {
            Ok(emb) => Some(emb),
            Err(e) => {
                tracing::warn!("embedding failed for recall: {}", e);
                None
            }
        }
    } else {
        None
    };

    let semantic_req = SearchRequest {
        query: body.query.clone(),
        embedding: query_embedding,
        limit: Some(limit),
        category: None,
        source: None,
        tags: None,
        threshold: None,
        user_id: Some(user_id),
        space_id: body.space_id,
        include_forgotten: None,
    };
    let semantic_results = hybrid_search(&state.db, semantic_req).await?;

    // Bucket 3: High-importance recent memories
    let recent_opts = ListOptions {
        limit: 20,
        offset: 0,
        category: None,
        source: None,
        user_id: Some(user_id),
        space_id: body.space_id,
        include_forgotten: false,
        include_archived: false,
    };
    let recent_all = memory::list(&state.db, recent_opts).await?;
    let important_memories: Vec<_> = recent_all
        .into_iter()
        .filter(|m| m.importance >= 7)
        .take(10)
        .collect();

    // Collect all memory IDs we already have
    let mut seen_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();

    // Breakdown counters
    let static_count = static_memories.len();
    let important_count = important_memories.len();

    // Build output memories list, tagging each with recall_source and recall_score
    let mut output: Vec<Value> = Vec::new();

    for m in &static_memories {
        if seen_ids.insert(m.id) {
            output.push(json!({
                "id": m.id,
                "content": m.content,
                "category": m.category,
                "recall_source": "static",
                "recall_score": m.importance as f64,
                "tags": parse_tags(&m.tags),
            }));
        }
    }

    for m in &important_memories {
        if seen_ids.insert(m.id) {
            output.push(json!({
                "id": m.id,
                "content": m.content,
                "category": m.category,
                "recall_source": "important",
                "recall_score": m.importance as f64,
                "tags": parse_tags(&m.tags),
            }));
        }
    }

    // FTS hits: fetch memories for each hit
    let mut semantic_count = 0usize;
    let mut recent_count = 0usize;

    for r in &semantic_results {
        if seen_ids.insert(r.memory.id) {
            semantic_count += 1;
            output.push(json!({
                "id": r.memory.id,
                "content": r.memory.content,
                "category": r.memory.category,
                "recall_source": "semantic",
                "recall_score": r.score,
                "tags": parse_tags(&r.memory.tags),
            }));
        }
    }

    // Recent memories (non-static, lower importance) from the full recent list
    let recent_extra_opts = ListOptions {
        limit: 10,
        offset: 0,
        category: None,
        source: None,
        user_id: Some(user_id),
        space_id: body.space_id,
        include_forgotten: false,
        include_archived: false,
    };
    let recent_extra = memory::list(&state.db, recent_extra_opts).await?;
    for m in recent_extra.iter().filter(|m| m.importance < 7 && !m.is_static) {
        if seen_ids.insert(m.id) {
            recent_count += 1;
            output.push(json!({
                "id": m.id,
                "content": m.content,
                "category": m.category,
                "recall_source": "recent",
                "recall_score": m.importance as f64,
                "tags": parse_tags(&m.tags),
            }));
        }
    }

    // Truncate to limit
    output.truncate(limit);
    let count = output.len();

    Ok(Json(json!({
        "memories": output,
        "breakdown": {
            "static": static_count,
            "semantic": semantic_count,
            "important": important_count,
            "recent": recent_count,
        },
        "count": count,
    })))
}

// ---------------------------------------------------------------------------
// GET /list
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ListQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub category: Option<String>,
    pub source: Option<String>,
    pub space_id: Option<i64>,
    pub include_forgotten: Option<bool>,
    pub include_archived: Option<bool>,
}

async fn list_memories(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let opts = ListOptions {
        limit: params.limit.unwrap_or(50),
        offset: params.offset.unwrap_or(0),
        category: params.category,
        source: params.source,
        user_id: Some(auth.user_id),
        space_id: params.space_id,
        include_forgotten: params.include_forgotten.unwrap_or(false),
        include_archived: params.include_archived.unwrap_or(false),
    };

    let memories = memory::list(&state.db, opts).await?;
    let results: Vec<Value> = memories.iter().map(memory_to_json).collect();

    Ok(Json(json!({ "results": results })))
}

// ---------------------------------------------------------------------------
// GET /memory/{id}
// ---------------------------------------------------------------------------

async fn get_memory(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let mem = memory::get(&state.db, id).await?;
    Ok(Json(memory_to_json(&mem)))
}

// ---------------------------------------------------------------------------
// DELETE /memory/{id}
// ---------------------------------------------------------------------------

async fn delete_memory(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    memory::delete(&state.db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

// ---------------------------------------------------------------------------
// POST /memory/{id}/update
// ---------------------------------------------------------------------------

async fn update_memory(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<Value>, AppError> {
    let updated = memory::update(&state.db, id, req).await?;
    Ok(Json(memory_to_json(&updated)))
}
