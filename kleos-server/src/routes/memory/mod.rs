use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post, put},
    Json, Router,
};
use base64::Engine;
use kleos_lib::artifacts::{self, ArtifactSummary, StoreArtifactOpts};
use kleos_lib::graph::entities::extract_and_link_entities;
use kleos_lib::intelligence::extraction::fast_extract_facts;
use kleos_lib::memory::{
    self,
    search::{faceted_search, hybrid_search, hybrid_search_reranked},
    types::{FacetedSearchRequest, ListOptions, SearchRequest, StoreRequest, UpdateRequest},
};
use rusqlite::params;
use serde_json::{json, Value};
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;

use crate::{
    brain_absorber::absorb_activity_to_brain,
    error::AppError,
    extractors::{Auth, ResolvedDb},
    routes::fsrs::record_recall_good,
    state::AppState,
};

mod types;
use types::{
    ForgetBody, ListQuery, RecallBody, SearchBody, SearchTagsBody, TrashListOptions, UpdateTagsBody,
};

/// Mount the memory router with the full set of CRUD, search, recall, tag, profile, and version-chain routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/store", post(store_memory))
        .route("/memory", post(store_memory))
        .route("/memories", post(store_memory))
        .route("/search", post(search_memories))
        .route("/memories/search", post(search_memories))
        .route("/search/explain", post(explain_search))
        .route("/recall", post(recall))
        .route("/list", get(list_memories))
        .route("/tags", get(list_tags))
        .route("/tags/search", post(search_tags))
        .route("/search/faceted", post(faceted_search_handler))
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
        // First use of a tenant shard may run migrations before the handler
        // body executes. Keep this high enough that cold shards do not return
        // 408 while hot search/recall paths still have a hard cap.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(60),
        ))
        // S7-27: memory payloads are small JSON; 256 KB covers any realistic content.
        .layer(DefaultBodyLimit::max(256 * 1024))
}

/// Decode the optional comma-separated tag string on a stored memory row into a Vec<String>.
fn parse_tags(tags: &Option<String>) -> Vec<String> {
    tags.as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .unwrap_or_default()
}

/// Serialize a Memory to the JSON shape the memory routes return on the wire.
fn memory_to_json(m: &kleos_lib::memory::types::Memory) -> Value {
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

/// POST /memory -- store a new memory and trigger background fact and entity extraction.
#[tracing::instrument(skip_all)]
async fn store_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(mut req): Json<StoreRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if req.content.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "content must not be empty".to_string(),
        )));
    }

    req.user_id = Some(auth.effective_user_id());
    let content = req.content.clone();
    let brain_category = req.category.clone();
    let brain_source = req.source.clone();
    let brain_importance = req.importance as f64;
    let inline_artifacts = req.artifacts.take();
    let embedder = state.current_embedder().await;
    let pre_embedded = req.embedding.is_some();
    let result = if let Some(ref e) = embedder {
        memory::store_with_chunks(&db, e.as_ref(), req).await?
    } else {
        memory::store(&db, req, None, false).await?
    };
    let embedded = pre_embedded || embedder.is_some();
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

    // Process inline artifact attachments (max 10 per store call).
    let mut artifact_summaries: Vec<ArtifactSummary> = Vec::new();
    if let Some(ref inline_arts) = inline_artifacts {
        if inline_arts.len() > 10 {
            return Err(AppError(kleos_lib::EngError::InvalidInput(
                "at most 10 inline artifacts per store call".into(),
            )));
        }
        for art in inline_arts {
            if art.filename.is_empty() {
                return Err(AppError(kleos_lib::EngError::InvalidInput(
                    "inline artifact filename must not be empty".into(),
                )));
            }
            if art.data_base64.is_empty() {
                return Err(AppError(kleos_lib::EngError::InvalidInput(
                    "inline artifact data_base64 must not be empty".into(),
                )));
            }
            let data = base64::engine::general_purpose::STANDARD
                .decode(&art.data_base64)
                .map_err(|e| {
                    AppError(kleos_lib::EngError::InvalidInput(format!(
                        "invalid base64 in artifact '{}': {e}",
                        art.filename
                    )))
                })?;
            let mime = art
                .mime_type
                .clone()
                .unwrap_or_else(|| "application/octet-stream".to_string());
            let size_bytes = data.len() as i64;
            let sha256 = artifacts::sha256_hex(&data);
            let indexable_content = artifacts::extract_indexable_content(&mime, &data);
            let opts = StoreArtifactOpts {
                content: indexable_content,
                ..StoreArtifactOpts::default()
            };
            let art_id = artifacts::store_artifact(
                &db,
                result.id,
                &art.filename,
                &art.filename,
                &mime,
                size_bytes,
                &sha256,
                "inline",
                Some(data),
                None,
                false,
                &opts,
            )
            .await?;
            artifact_summaries.push(ArtifactSummary {
                id: art_id,
                filename: art.filename.clone(),
                mime_type: mime,
                size_bytes,
            });
        }
    }

    // Background: extract facts, preferences, and state from the new memory.
    // Bounded by fact_extract_sem (H-005); shutdown-propagated via shutdown_token (M-008).
    {
        let db = db.clone();
        let memory_id = result.id;
        let user_id = auth.effective_user_id();
        // Clone content before consuming it in the spawn so the entity_extract
        // block below can use the same value without a borrow conflict.
        let content_for_extract = content.clone();
        let permit = match state.fact_extract_sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!("fact_extract semaphore closed; skipping background work");
                let mem = memory::get(&db, result.id, auth.effective_user_id()).await?;
                return Ok((
                    StatusCode::CREATED,
                    Json(json!({
                        "stored": true, "id": result.id, "created_at": mem.created_at,
                        "importance": mem.importance, "embedded": embedded,
                        "tags": parse_tags(&mem.tags),
                        "decay_score": mem.decay_score.unwrap_or(mem.importance as f64),
                    })),
                ));
            }
        };
        let shutdown = state.shutdown_token.clone();
        let mut bg = state.background_tasks.lock().await;
        bg.spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("background fact_extract drained on shutdown");
                }
                _ = async {
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
                } => {}
            }
        });
    }

    let content_for_brain = content.clone();

    // Background: extract and link named entities from the new memory.
    // Uses the same fact_extract_sem semaphore (H-005) and shutdown token (M-008).
    // Runs in a separate spawn from fact_extract so a failure in one does not
    // affect the other -- independent error blast radius.
    {
        let db = db.clone();
        let memory_id = result.id;
        let user_id = auth.effective_user_id();
        // Move content into this closure; it is no longer needed after this block.
        let content_for_entities = content;
        let permit = match state.fact_extract_sem.clone().acquire_owned().await {
            Ok(p) => p,
            Err(_) => {
                tracing::warn!("fact_extract semaphore closed; skipping entity extraction");
                // Semaphore closed means server is shutting down; skip extraction
                // and fall through to return the stored memory as normal.
                let mem = memory::get(&db, result.id, auth.effective_user_id()).await?;
                return Ok((
                    StatusCode::CREATED,
                    Json(json!({
                        "stored": true, "id": result.id, "created_at": mem.created_at,
                        "importance": mem.importance, "embedded": embedded,
                        "tags": parse_tags(&mem.tags),
                        "decay_score": mem.decay_score.unwrap_or(mem.importance as f64),
                    })),
                ));
            }
        };
        let shutdown = state.shutdown_token.clone();
        let mut bg = state.background_tasks.lock().await;
        bg.spawn(async move {
            let _permit = permit;
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::debug!("background entity_extract drained on shutdown");
                }
                _ = async {
                    match extract_and_link_entities(&db, memory_id, &content_for_entities, user_id).await {
                        Ok(entities) => {
                            if !entities.is_empty() {
                                tracing::debug!(
                                    memory_id,
                                    entity_count = entities.len(),
                                    "auto entity extraction completed"
                                );
                            }
                        }
                        Err(e) => tracing::warn!(memory_id, "auto entity extraction failed: {}", e),
                    }
                } => {}
            }
        });
    }

    // Background: absorb new memory into the Hopfield brain.
    // Fire-and-forget, best-effort — never fails the store response.
    // Bounded by brain_absorb_sem (H-005); shutdown-propagated via shutdown_token (M-008).
    if let Some(brain) = state.brain.clone() {
        let embedder = state.embedder.clone();
        let memory_id = result.id;
        // Act-as aware: absorb under the effective (delegated) user -- the same
        // owner the memory was stored under (req.user_id) -- not the real caller,
        // matching the sibling brain blocks and the brain's per-user partitioning.
        let user_id = auth.effective_user_id();
        match state.brain_absorb_sem.clone().acquire_owned().await {
            Ok(permit) => {
                let shutdown = state.shutdown_token.clone();
                let mut bg = state.background_tasks.lock().await;
                bg.spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        _ = shutdown.cancelled() => {
                            tracing::debug!("background brain_absorb drained on shutdown");
                        }
                        _ = absorb_activity_to_brain(
                            brain, embedder, user_id, memory_id, content_for_brain,
                            brain_category, brain_importance, brain_source,
                        ) => {}
                    }
                });
            }
            Err(_) => tracing::warn!("brain_absorb semaphore closed; skipping brain absorption"),
        }
    }

    let mem = memory::get(&db, result.id, auth.effective_user_id()).await?;
    let mut response = json!({
        "stored": true, "id": result.id, "created_at": mem.created_at,
        "importance": mem.importance, "embedded": embedded,
        "tags": parse_tags(&mem.tags),
        "decay_score": mem.decay_score.unwrap_or(mem.importance as f64),
    });
    if !artifact_summaries.is_empty() {
        response["artifacts"] = json!(artifact_summaries);
    }
    Ok((StatusCode::CREATED, Json(response)))
}

/// POST /search -- hybrid keyword + semantic memory search.
#[tracing::instrument(skip_all)]
async fn search_memories(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SearchBody>,
) -> Result<Json<Value>, AppError> {
    let embedding = {
        if let Some(embedder) = state.current_embedder().await {
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
        user_id: Some(auth.effective_user_id()),
        space_id: body.space_id,
        include_forgotten: body.include_forgotten,
        mode: body.mode,
        question_type: body.question_type,
        expand_relationships: body.expand_relationships.unwrap_or(false),
        include_links: body.include_links.unwrap_or(false),
        latest_only: body.latest_only.unwrap_or(true),
        source_filter: body.source_filter,
        budget: body.budget,
        ..Default::default()
    };

    // SEC-recall-1.5: route the rerank through the library wrapper so any
    // future in-process caller (context, MCP, sidecar) gets the same blend
    // by supplying a reranker. The route still pulls the reranker from
    // AppState; the wrapper handles the None case as a no-op.
    let reranker = state.current_reranker().await;
    let arc_results = hybrid_search_reranked(&db, req, &body_query, reranker).await?;
    let results = (*arc_results).clone();

    let top_score = results.first().map(|r| r.score).unwrap_or(0.0);
    let abstained = results.is_empty();

    // Batch-load artifact summaries for all returned memories.
    let memory_ids: Vec<i64> = results.iter().map(|r| r.memory.id).collect();
    let artifact_map = artifacts::enrich_with_artifacts(&db, &memory_ids)
        .await
        .unwrap_or_default();

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
            // SEC-recall-1.6: surface the per-channel breakdown that the
            // backend SearchResult already carries. Operators previously saw
            // only the compound `score` (RRF * decay * boosts), which
            // collapses recall signal into a narrow band. Each field stays
            // omitted when None so the wire shape remains compact.
            if let Some(s) = r.semantic_score {
                item["semantic_score"] = json!(s);
            }
            if let Some(s) = r.fts_score {
                item["fts_score"] = json!(s);
            }
            if let Some(s) = r.graph_score {
                item["graph_score"] = json!(s);
            }
            if let Some(s) = r.combined_score {
                item["combined_score"] = json!(s);
            }
            if let Some(s) = r.temporal_boost {
                item["temporal_boost"] = json!(s);
            }
            if let Some(s) = r.personality_signal_score {
                item["personality_signal_score"] = json!(s);
            }
            if let Some(ref linked) = r.linked {
                item["linked"] = json!(linked);
            }
            if let Some(ref vc) = r.version_chain {
                item["version_chain"] = json!(vc);
            }
            item["artifacts"] = json!(artifact_map.get(&r.memory.id).cloned().unwrap_or_default());
            item
        })
        .collect();

    Ok(Json(json!({
        "results": result_items, "abstained": abstained, "top_score": top_score,
    })))
}

/// Part 5.13: POST /search/explain -- runs the full hybrid search pipeline and
/// returns a per-result score breakdown (lexical/vector/graph/reranker/fused)
/// alongside stage timings so operators can diagnose ranking regressions.
#[tracing::instrument(skip_all)]
async fn explain_search(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SearchBody>,
) -> Result<Json<Value>, AppError> {
    let total_start = std::time::Instant::now();

    let embed_start = std::time::Instant::now();
    let embedding = {
        if let Some(embedder) = state.current_embedder().await {
            match embedder.embed(&body.query).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!("embedding failed for explain: {}", e);
                    None
                }
            }
        } else {
            None
        }
    };
    let embed_ms = embed_start.elapsed().as_secs_f64() * 1000.0;
    let embedded = embedding.is_some();

    let body_query = body.query.clone();
    let limit = body.limit.map(|l| l.min(100));

    let req = SearchRequest {
        query: body.query,
        embedding,
        limit,
        category: body.category,
        source: body.source,
        tags: body.tags.or_else(|| body.tag.map(|tag| vec![tag])),
        threshold: body.threshold,
        user_id: Some(auth.effective_user_id()),
        space_id: body.space_id,
        include_forgotten: body.include_forgotten,
        mode: body.mode.clone(),
        question_type: body.question_type,
        expand_relationships: body.expand_relationships.unwrap_or(false),
        include_links: body.include_links.unwrap_or(false),
        latest_only: body.latest_only.unwrap_or(true),
        source_filter: body.source_filter,
        budget: body.budget,
        ..Default::default()
    };

    let hybrid_start = std::time::Instant::now();
    let arc_results = hybrid_search(&db, req).await?;
    let mut results = (*arc_results).clone();
    let hybrid_ms = hybrid_start.elapsed().as_secs_f64() * 1000.0;

    let rerank_start = std::time::Instant::now();
    let mut reranker_applied = false;
    {
        let reranker_guard = state.reranker.read().await;
        if let Some(ref reranker) = *reranker_guard {
            match reranker.rerank_results(&body_query, &mut results).await {
                Ok(()) => reranker_applied = true,
                Err(e) => tracing::warn!("reranker failed for explain: {}", e),
            }
        }
    }
    let rerank_ms = rerank_start.elapsed().as_secs_f64() * 1000.0;

    let result_items: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "id": r.memory.id,
                "content": r.memory.content,
                "score": r.score,
                "search_type": r.search_type,
                "scores": {
                    "lexical": r.fts_score,
                    "vector": r.semantic_score,
                    "graph": r.graph_score,
                    "personality": r.personality_signal_score,
                    "temporal_boost": r.temporal_boost,
                    "fused": r.combined_score,
                    "reranked": r.reranked.unwrap_or(false),
                    "reranker_ms": r.reranker_ms,
                },
                "multipliers": {
                    "rrf": r.rrf_pre_boost,
                    "decay": r.decay_factor,
                    "pagerank": r.pr_boost,
                    "source_count": r.src_boost,
                    "static": r.stat_boost,
                    "contradiction": r.contradiction,
                },
            })
        })
        .collect();

    let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;

    Ok(Json(json!({
        "results": result_items,
        "count": result_items.len(),
        "timings_ms": {
            "embed": embed_ms,
            "hybrid": hybrid_ms,
            "rerank": rerank_ms,
            "total": total_ms,
        },
        "pipeline": {
            "embedded": embedded,
            "reranker_applied": reranker_applied,
            "mode": body.mode,
        },
    })))
}

/// POST /recall -- retrieve memories ranked by importance and recency.
#[tracing::instrument(skip_all)]
async fn recall(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20).min(100);
    let user_id = auth.effective_user_id();
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
    let all_list = memory::list(&db, static_opts).await?;
    let static_memories: Vec<_> = all_list.into_iter().filter(|m| m.is_static).collect();

    let query_embedding = {
        if let Some(embedder) = state.current_embedder().await {
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
        user_id: Some(user_id),
        space_id: body.space_id,
        ..Default::default()
    };
    let semantic_results = hybrid_search(&db, semantic_req).await?;

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
    let recent_all = memory::list(&db, recent_opts).await?;
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
    for r in semantic_results.iter() {
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
    let recent_extra = memory::list(&db, recent_extra_opts).await?;
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

    // Background: update FSRS state (grade=Good) for every recalled memory.
    // Fire-and-forget — never delays or fails the recall response.
    {
        let recalled_ids: Vec<i64> = output.iter().filter_map(|v| v["id"].as_i64()).collect();
        let db_clone = db.clone();
        tokio::spawn(async move {
            for id in recalled_ids {
                record_recall_good(&db_clone, id).await;
            }
        });
    }

    // Batch-load artifact summaries for all recalled memories.
    let recall_ids: Vec<i64> = output.iter().filter_map(|v| v["id"].as_i64()).collect();
    let recall_art_map = artifacts::enrich_with_artifacts(&db, &recall_ids)
        .await
        .unwrap_or_default();
    for item in &mut output {
        if let Some(mid) = item["id"].as_i64() {
            item["artifacts"] = json!(recall_art_map.get(&mid).cloned().unwrap_or_default());
        }
    }

    let count = output.len();

    // Build compat profile from static memories for legacy clients
    let profile: Vec<&str> = static_memories.iter().map(|m| m.content.as_str()).collect();
    let results = output.clone();

    Ok(Json(json!({
        "memories": output,
        "results": results,
        "profile": profile,
        "breakdown": { "static": static_count, "semantic": semantic_count,
                       "important": important_count, "recent": recent_count },
        "count": count,
    })))
}

/// GET /list -- paginated listing of stored memories.
#[tracing::instrument(skip_all)]
async fn list_memories(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(params): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let opts = ListOptions {
        limit: params.limit.unwrap_or(50).min(1000),
        offset: params.offset.unwrap_or(0),
        category: params.category,
        source: params.source,
        user_id: Some(auth.effective_user_id()),
        space_id: params.space_id,
        include_forgotten: params.include_forgotten.unwrap_or(false),
        include_archived: params.include_archived.unwrap_or(false),
    };
    let memories = memory::list(&db, opts).await?;
    let results: Vec<Value> = memories.iter().map(memory_to_json).collect();
    Ok(Json(json!({ "results": results })))
}

/// GET /memory/{id} -- fetch a single memory by id.
#[tracing::instrument(skip_all)]
async fn get_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let mem = memory::get(&db, id, auth.effective_user_id()).await?;
    Ok(Json(memory_to_json(&mem)))
}

/// DELETE /memory/{id} -- soft-delete a memory.
#[tracing::instrument(skip_all)]
async fn delete_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    memory::delete(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

/// GET /memory/trashed -- list memories that have been soft-deleted but are still recoverable.
#[tracing::instrument(skip_all)]
async fn list_trashed(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Query(opts): Query<TrashListOptions>,
) -> Result<Json<Value>, AppError> {
    let limit = opts.limit.unwrap_or(50).min(200);
    let memories = memory::list_trashed(&db, auth.effective_user_id(), limit).await?;
    let items: Vec<Value> = memories.iter().map(memory_to_json).collect();
    Ok(Json(json!({ "memories": items, "count": items.len() })))
}

/// POST /memory/{id}/restore -- restore a soft-deleted memory back to the active corpus.
#[tracing::instrument(skip_all)]
async fn restore_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let restored = memory::restore(&db, id, auth.effective_user_id()).await?;
    Ok(Json(memory_to_json(&restored)))
}

/// PUT /memory/{id} -- update fields on an existing memory.
#[tracing::instrument(skip_all)]
async fn update_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(req): Json<UpdateRequest>,
) -> Result<Json<Value>, AppError> {
    let updated = memory::update(&db, id, req, auth.effective_user_id(), false).await?;
    Ok(Json(memory_to_json(&updated)))
}

/// GET /tags -- list all tags in use across the corpus.
#[tracing::instrument(skip_all)]
async fn list_tags(Auth(auth): Auth, ResolvedDb(db): ResolvedDb) -> Result<Json<Value>, AppError> {
    let tags = memory::list_all_tags(&db, auth.effective_user_id()).await?;
    Ok(Json(json!({ "tags": tags })))
}

/// POST /tags/search -- search tags by prefix and return matching memory counts.
#[tracing::instrument(skip_all)]
async fn search_tags(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<SearchTagsBody>,
) -> Result<Json<Value>, AppError> {
    if body.tags.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "tags must not be empty".to_string(),
        )));
    }

    let memories = memory::search_by_tags(
        &db,
        auth.effective_user_id(),
        &body.tags,
        body.match_all.unwrap_or(false),
        body.limit.unwrap_or(50).min(100),
    )
    .await?;
    let results: Vec<Value> = memories.iter().map(memory_to_json).collect();
    Ok(Json(json!({ "results": results })))
}

// 3.11: POST /search/faceted -- structured filter + facet aggregation
#[tracing::instrument(skip_all)]
async fn faceted_search_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(mut body): Json<FacetedSearchRequest>,
) -> Result<Json<Value>, AppError> {
    body.user_id = Some(auth.effective_user_id());
    body.limit = body.limit.min(100);

    // Embed query if present.
    if !body.query.is_empty() {
        if let Some(embedder) = state.current_embedder().await {
            match embedder.embed(&body.query).await {
                Ok(emb) => body.embedding = Some(emb),
                Err(e) => tracing::warn!("embedding failed for faceted search: {}", e),
            }
        }
    }

    let resp = faceted_search(&db, body).await?;
    Ok(Json(json!(resp)))
}

/// PUT /memory/{id}/tags -- replace the tag set on a memory.
#[tracing::instrument(skip_all)]
async fn update_tags(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    Json(body): Json<UpdateTagsBody>,
) -> Result<Json<Value>, AppError> {
    memory::update_memory_tags(&db, id, auth.effective_user_id(), &body.tags).await?;
    let updated = memory::get(&db, id, auth.effective_user_id()).await?;
    Ok(Json(memory_to_json(&updated)))
}

/// GET /memory/profile -- return the stored user profile synthesized from memories.
#[tracing::instrument(skip_all)]
async fn profile_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let profile = memory::get_user_profile(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(profile)))
}

/// POST /memory/profile/synthesize -- rebuild the user profile from recent memories.
#[tracing::instrument(skip_all)]
async fn synthesize_profile(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let uid = auth.effective_user_id();
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM personality_signals WHERE user_id = ?1 AND memory_id IS NOT NULL",
            params![uid],
        )?;
        Ok(())
    })
    .await?;

    let memories = memory::list(
        &db,
        ListOptions {
            limit: 200,
            offset: 0,
            category: None,
            source: None,
            user_id: Some(auth.effective_user_id()),
            space_id: None,
            include_forgotten: false,
            include_archived: true,
        },
    )
    .await?;

    for mem in &memories {
        let _ = kleos_lib::personality::extract_personality_signals(
            &db,
            &mem.content,
            mem.id,
            auth.effective_user_id(),
        )
        .await?;
    }

    let _ = kleos_lib::personality::synthesize_personality_profile(&db, auth.effective_user_id())
        .await?;
    let profile = memory::get_user_profile(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(profile)))
}

/// GET /memory/stats -- counts and aggregates for the calling user's memories.
#[tracing::instrument(skip_all)]
async fn user_stats(Auth(auth): Auth, ResolvedDb(db): ResolvedDb) -> Result<Json<Value>, AppError> {
    let stats = memory::get_user_stats(&db, auth.effective_user_id()).await?;
    Ok(Json(json!(stats)))
}

/// POST /memory/{id}/forget -- mark a memory as forgotten so it is hidden from search.
#[tracing::instrument(skip_all)]
async fn forget_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
    body: Option<Json<ForgetBody>>,
) -> Result<Json<Value>, AppError> {
    memory::mark_forgotten(&db, id, auth.effective_user_id()).await?;
    if let Some(reason) = body.and_then(|Json(body)| body.reason) {
        memory::update_forget_reason(&db, id, &reason, auth.effective_user_id()).await?;
    }
    Ok(Json(json!({ "id": id, "status": "forgotten" })))
}

/// POST /memory/{id}/archive -- move a memory out of the active corpus into the archive.
#[tracing::instrument(skip_all)]
async fn archive_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    memory::mark_archived(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "id": id, "status": "archived" })))
}

/// POST /memory/{id}/unarchive -- restore a memory from the archive back to the active corpus.
#[tracing::instrument(skip_all)]
async fn unarchive_memory(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    memory::mark_unarchived(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "id": id, "status": "active" })))
}

/// GET /memory/{id}/links -- list memory-to-memory links recorded for a given memory.
#[tracing::instrument(skip_all)]
async fn get_links(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let _ = memory::get(&db, id, auth.effective_user_id()).await?;
    let links = memory::get_links_for(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "links": links })))
}

/// GET /memory/{id}/versions -- return the full version chain rooted at this memory.
#[tracing::instrument(skip_all)]
async fn version_chain_handler(
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let versions = memory::get_version_chain(&db, id, auth.effective_user_id()).await?;
    Ok(Json(json!({ "versions": versions })))
}
