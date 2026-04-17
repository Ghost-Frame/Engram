use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use engram_lib::intelligence::extraction::fast_extract_facts;
use engram_lib::memory::{
    self,
    search::hybrid_search,
    types::{ListOptions, SearchRequest, StoreRequest, UpdateRequest},
};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/store", post(store_memory))
        .route("/memory", post(store_memory))
        .route("/memories", post(store_memory))
        .route("/search", post(search_memories))
        .route("/memories/search", post(search_memories))
        .route("/recall", post(recall))
        .route("/list", get(list_memories))
        .route("/tags", get(list_tags))
        .route("/tags/search", post(search_tags))
        .route("/profile", get(profile_handler))
        .route("/profile/synthesize", post(synthesize_profile))
        .route("/me/stats", get(user_stats))
        .route("/links/{id}", get(get_links))
        .route("/versions/{id}", get(version_chain_handler))
        .route("/memory/{id}", get(get_memory).delete(delete_memory))
        .route("/memory/{id}/update", post(update_memory))
        .route("/memory/{id}/tags", put(update_tags))
        .route("/memory/{id}/forget", post(forget_memory))
        .route("/memory/{id}/archive", post(archive_memory))
        .route("/memory/{id}/unarchive", post(unarchive_memory))
        .route("/memory/{id}/restore", post(restore_memory))
        .route("/memory/trash", get(list_trashed))
        // S7-26: search/recall is DB + embedding lookup; 10s is generous.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(10),
        ))
        // S7-27: memory payloads are small JSON; 256 KB covers any realistic content.
        .layer(DefaultBodyLimit::max(256 * 1024))
}

fn parse_tags(tags: &Option<String>) -> Vec<String> {
    tags.as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

fn memory_to_json(m: &engram_lib::memory::types::Memory) -> Value {
    json!({
        "id": m.id, "content": m.content, "category": m.category,
        "source": m.source, "session_id": m.session_id, "importance": m.importance,
        "version": m.version, "is_latest": m.is_latest,
        "parent_memory_id": m.parent_memory_id, "root_memory_id": m.root_memory_id,
        "source_count": m.source_count, "is_static": m.is_static,
        "is_forgotten": m.is_forgotten, "is_archived": m.is_archived,
        "is_fact": m.is_fact,
        "is_decomposed": m.is_decomposed, "forget_after": m.forget_after,
        "forget_reason": m.forget_reason, "model": m.model,
        "recall_hits": m.recall_hits, "recall_misses": m.recall_misses,
        "adaptive_score": m.adaptive_score, "pagerank_score": m.pagerank_score,
        "last_accessed_at": m.last_accessed_at, "access_count": m.access_count,
        "tags": parse_tags(&m.tags), "episode_id": m.episode_id,
        "decay_score": m.decay_score, "confidence": m.confidence,
        "sync_id": m.sync_id, "status": m.status,
        "user_id": m.user_id, "space_id": m.space_id,
        "created_at": m.created_at, "updated_at": m.updated_at,
    })
}

async fn store_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(mut req): Json<StoreRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if req.content.trim().is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "content must not be empty".to_string(),
        )));
    }

    req.user_id = Some(auth.user_id);
    if req.embedding.is_none() {
        let embedder_guard = state.embedder.read().await;
        if let Some(ref embedder) = *embedder_guard {
            match embedder.embed(&req.content).await {
                Ok(emb) => req.embedding = Some(emb),
                Err(e) => tracing::warn!("embedding failed for store: {}", e),
            }
        }
    }
    let embedded = req.embedding.is_some();
    let content = req.content.clone();
    let result = memory::store(&state.db, req).await?;
    if let Some(existing_id) = result.duplicate_of {
        return Ok((
            StatusCode::OK,
            Json(json!({
                "stored": false, "duplicate": true,
                "existing_id": existing_id, "boosted": true,
                "distance": Value::Null,
            })),
        ));
    }

    // Background: extract facts, preferences, and state from the new memory.
    // Fire-and-forget so the store response is not delayed.
    {
        let db = state.db.clone();
        let memory_id = result.id;
        let user_id = auth.user_id;
        let content_for_extract = content;
        tokio::spawn(async move {
            match fast_extract_facts(&db, &content_for_extract, memory_id, user_id, None).await {
                Ok(stats) => {
                    let total = stats.facts + stats.preferences + stats.state_updates;
                    if total > 0 {
                        tracing::debug!(
                            memory_id,
                            facts = stats.facts,
                            prefs = stats.preferences,
                            states = stats.state_updates,
                            "auto-extraction completed"
                        );
                    }
                }
                Err(e) => tracing::warn!(memory_id, "auto-extraction failed: {}", e),
            }
        });
    }

    let mem = memory::get(&state.db, result.id, auth.user_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "stored": true, "id": result.id, "created_at": mem.created_at,
            "importance": mem.importance, "embedded": embedded,
            "tags": parse_tags(&mem.tags),
            "decay_score": mem.decay_score.unwrap_or(mem.importance as f64),
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct SearchBody {
    pub query: String,
    pub limit: Option<usize>,
    pub category: Option<String>,
    pub source: Option<String>,
    pub tags: Option<Vec<String>>,
    pub threshold: Option<f32>,
    pub tag: Option<String>,
    pub space_id: Option<i64>,
    pub include_forgotten: Option<bool>,
    pub mode: Option<String>,
    pub question_type: Option<engram_lib::memory::types::QuestionType>,
    pub expand_relationships: Option<bool>,
    pub include_links: Option<bool>,
    pub latest_only: Option<bool>,
    pub source_filter: Option<String>,
}

async fn search_memories(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SearchBody>,
) -> Result<Json<Value>, AppError> {
    let embedding = {
        let embedder_guard = state.embedder.read().await;
        if let Some(ref embedder) = *embedder_guard {
            match embedder.embed(&body.query).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!("embedding failed for search: {}", e);
                    None
                }
            }
        } else {
            None
        }
    };

    let body_query = body.query.clone();

    // Cap limit to prevent DoS via unbounded result sets
    let limit = body.limit.map(|l| l.min(100));

    let req = SearchRequest {
        query: body.query,
        embedding,
        limit,
        category: body.category,
        source: body.source,
        tags: body.tags.or_else(|| body.tag.map(|tag| vec![tag])),
        threshold: body.threshold,
        user_id: Some(auth.user_id),
        space_id: body.space_id,
        include_forgotten: body.include_forgotten,
        mode: body.mode,
        question_type: body.question_type,
        expand_relationships: body.expand_relationships.unwrap_or(false),
        include_links: body.include_links.unwrap_or(false),
        latest_only: body.latest_only.unwrap_or(true),
        source_filter: body.source_filter,
    };

    let mut results = hybrid_search(&state.db, req).await?;

    {
        let reranker_guard = state.reranker.read().await;
        if let Some(ref reranker) = *reranker_guard {
            if let Err(e) = reranker.rerank_results(&body_query, &mut results).await {
                tracing::warn!("reranker failed, using original order: {}", e);
            }
        }
    }

    let top_score = results.first().map(|r| r.score).unwrap_or(0.0);
    let abstained = results.is_empty();

    let result_items: Vec<Value> = results
        .iter()
        .map(|r| {
            let mut item = json!({
                "id": r.memory.id, "content": r.memory.content,
                "category": r.memory.category, "source": r.memory.source,
                "importance": r.memory.importance, "created_at": r.memory.created_at,
                "score": r.score, "tags": parse_tags(&r.memory.tags),
                "search_type": r.search_type,
            });
            if let Some(d) = r.decay_score {
                item["decay_score"] = json!(d);
            }
            if let Some(q) = &r.question_type {
                item["question_type"] = json!(q);
            }
            if let Some(ref ch) = r.channels {
                item["channels"] = json!(ch);
            }
            if let Some(ref linked) = r.linked {
                item["linked"] = json!(linked);
            }
            if let Some(ref vc) = r.version_chain {
                item["version_chain"] = json!(vc);
            }
            item
        })
        .collect();

    Ok(Json(json!({
        "results": result_items, "abstained": abstained, "top_score": top_score,
    })))
}

#[derive(Debug, Deserialize)]
struct RecallBody {
    pub context: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub space_id: Option<i64>,
}

async fn recall(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20).min(100);
    let user_id = auth.user_id;
    let query = body
        .query
        .filter(|q| !q.trim().is_empty())
        .or(body.context)
        .unwrap_or_default();

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
    let all_list = memory::list(&state.db, static_opts).await?;
    let static_memories: Vec<_> = all_list.into_iter().filter(|m| m.is_static).collect();

    let query_embedding = {
        let embedder_guard = state.embedder.read().await;
        if let Some(ref embedder) = *embedder_guard {
            match embedder.embed(&query).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!("embedding failed for recall: {}", e);
                    None
                }
            }
        } else {
            None
        }
    };

    let semantic_req = SearchRequest {
        query: query.clone(),
        embedding: query_embedding,
        limit: Some(limit),
        category: None,
        source: None,
        tags: None,
        threshold: None,
        user_id: Some(user_id),
        space_id: body.space_id,
        include_forgotten: None,
        mode: None,
        question_type: None,
        expand_relationships: false,
        include_links: false,
        latest_only: true,
        source_filter: None,
    };
    let semantic_results = hybrid_search(&state.db, semantic_req).await?;

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

    let mut seen_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let static_count = static_memories.len();
    let important_count = important_memories.len();
    let mut output: Vec<Value> = Vec::new();

    for m in &static_memories {
        if seen_ids.insert(m.id) {
            output.push(json!({
                "id": m.id, "content": m.content, "category": m.category,
                "recall_source": "static", "recall_score": m.importance as f64,
                "tags": parse_tags(&m.tags),
            }));
        }
    }
    for m in &important_memories {
        if seen_ids.insert(m.id) {
            output.push(json!({
                "id": m.id, "content": m.content, "category": m.category,
                "recall_source": "important", "recall_score": m.importance as f64,
                "tags": parse_tags(&m.tags),
            }));
        }
    }

    let mut semantic_count = 0usize;
    for r in &semantic_results {
        if seen_ids.insert(r.memory.id) {
            semantic_count += 1;
            output.push(json!({
                "id": r.memory.id, "content": r.memory.content,
                "category": r.memory.category, "recall_source": "semantic",
                "recall_score": r.score, "tags": parse_tags(&r.memory.tags),
            }));
        }
    }

    let mut recent_count = 0usize;
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
    for m in recent_extra
        .iter()
        .filter(|m| m.importance < 7 && !m.is_static)
    {
        if seen_ids.insert(m.id) {
            recent_count += 1;
            output.push(json!({
                "id": m.id, "content": m.content, "category": m.category,
                "recall_source": "recent", "recall_score": m.importance as f64,
                "tags": parse_tags(&m.tags),
            }));
        }
    }

    output.truncate(limit);
    let count = output.len();
    Ok(Json(json!({
        "memories": output,
        "breakdown": { "static": static_count, "semantic": semantic_count,
                       "important": important_count, "recent": recent_count },
        "count": count,
    })))
}

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

#[derive(Debug, Deserialize)]
struct TrashListOptions {
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SearchTagsBody {
    pub tags: Vec<String>,
    pub match_all: Option<bool>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct UpdateTagsBody {
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ForgetBody {
    pub reason: Option<String>,
}

async fn list_memories(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let opts = ListOptions {
        limit: params.limit.unwrap_or(50).min(1000),
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

async fn get_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let mem = memory::get(&state.db, id, auth.user_id).await?;
    Ok(Json(memory_to_json(&mem)))
}

async fn delete_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    memory::delete(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn list_trashed(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(opts): Query<TrashListOptions>,
) -> Result<Json<Value>, AppError> {
    let limit = opts.limit.unwrap_or(50).min(200);
    let memories = memory::list_trashed(&state.db, auth.user_id, limit).await?;
    let items: Vec<Value> = memories.iter().map(memory_to_json).collect();
    Ok(Json(json!({ "memories": items, "count": items.len() })))
}

async fn restore_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let restored = memory::restore(&state.db, id, auth.user_id).await?;
    Ok(Json(memory_to_json(&restored)))
}

async fn update_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<Value>, AppError> {
    let updated = memory::update(&state.db, id, req, auth.user_id).await?;
    Ok(Json(memory_to_json(&updated)))
}

async fn list_tags(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let tags = memory::list_all_tags(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "tags": tags })))
}

async fn search_tags(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<SearchTagsBody>,
) -> Result<Json<Value>, AppError> {
    if body.tags.is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "tags must not be empty".to_string(),
        )));
    }

    let memories = memory::search_by_tags(
        &state.db,
        auth.user_id,
        &body.tags,
        body.match_all.unwrap_or(false),
        body.limit.unwrap_or(50).min(100),
    )
    .await?;
    let results: Vec<Value> = memories.iter().map(memory_to_json).collect();
    Ok(Json(json!({ "results": results })))
}

async fn update_tags(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTagsBody>,
) -> Result<Json<Value>, AppError> {
    memory::update_memory_tags(&state.db, id, auth.user_id, &body.tags).await?;
    let updated = memory::get(&state.db, id, auth.user_id).await?;
    Ok(Json(memory_to_json(&updated)))
}

async fn profile_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let profile = memory::get_user_profile(&state.db, auth.user_id).await?;
    Ok(Json(json!(profile)))
}

async fn synthesize_profile(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let uid = auth.user_id;
    state
        .db
        .write(move |conn| {
            conn.execute(
                "DELETE FROM personality_signals WHERE user_id = ?1 AND memory_id IS NOT NULL",
                params![uid],
            )?;
            Ok(())
        })
        .await?;

    let memories = memory::list(
        &state.db,
        ListOptions {
            limit: 200,
            offset: 0,
            category: None,
            source: None,
            user_id: Some(auth.user_id),
            space_id: None,
            include_forgotten: false,
            include_archived: true,
        },
    )
    .await?;

    for mem in &memories {
        let _ = engram_lib::personality::extract_personality_signals(
            &state.db,
            &mem.content,
            mem.id,
            auth.user_id,
        )
        .await?;
    }

    let _ =
        engram_lib::personality::synthesize_personality_profile(&state.db, auth.user_id).await?;
    let profile = memory::get_user_profile(&state.db, auth.user_id).await?;
    Ok(Json(json!(profile)))
}

async fn user_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = memory::get_user_stats(&state.db, auth.user_id).await?;
    Ok(Json(json!(stats)))
}

async fn forget_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
    body: Option<Json<ForgetBody>>,
) -> Result<Json<Value>, AppError> {
    memory::mark_forgotten(&state.db, id, auth.user_id).await?;
    if let Some(reason) = body.and_then(|Json(body)| body.reason) {
        memory::update_forget_reason(&state.db, id, &reason, auth.user_id).await?;
    }
    Ok(Json(json!({ "id": id, "status": "forgotten" })))
}

async fn archive_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    memory::mark_archived(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "id": id, "status": "archived" })))
}

async fn unarchive_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    memory::mark_unarchived(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "id": id, "status": "active" })))
}

async fn get_links(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let _ = memory::get(&state.db, id, auth.user_id).await?;
    let links = memory::get_links_for(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "links": links })))
}

async fn version_chain_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let versions = memory::get_version_chain(&state.db, id, auth.user_id).await?;
    Ok(Json(json!({ "versions": versions })))
}
