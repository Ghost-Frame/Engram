use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::memory::search::hybrid_search;
use engram_lib::memory::types::{ListOptions, SearchRequest};
use engram_lib::{artifacts, conversations, episodes};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/search/unified", post(unified_search_handler))
        .route("/search/suggest", get(search_suggest_handler))
}

#[derive(Debug, Deserialize)]
struct UnifiedSearchBody {
    query: String,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct SuggestQuery {
    q: String,
    limit: Option<usize>,
}

async fn unified_search_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<UnifiedSearchBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.unwrap_or(20);

    let memory_results = hybrid_search(
        &state.db,
        SearchRequest {
            query: body.query.clone(),
            embedding: None,
            limit: Some(limit),
            category: None,
            source: None,
            tags: None,
            threshold: None,
            user_id: Some(auth.user_id),
            space_id: None,
            include_forgotten: Some(false),
            mode: None,
            question_type: None,
            expand_relationships: false,
            include_links: false,
            latest_only: true,
            source_filter: None,
        },
    )
    .await?;

    let episodes_results = episodes::list_episodes(&state.db, auth.user_id, limit).await?;
    let conversations_results = conversations::list_conversations(&state.db, auth.user_id, limit).await?;
    let artifact_results = artifacts::get_artifact_stats(&state.db, Some(auth.user_id)).await?;

    Ok(Json(json!({
        "query": body.query,
        "memories": memory_results,
        "episodes": episodes_results,
        "conversations": conversations_results,
        "artifacts": artifact_results,
    })))
}

async fn search_suggest_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(query): Query<SuggestQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = query.limit.unwrap_or(8);
    let list = engram_lib::memory::list(
        &state.db,
        ListOptions {
            limit,
            offset: 0,
            category: None,
            source: None,
            user_id: Some(auth.user_id),
            space_id: None,
            include_forgotten: false,
            include_archived: false,
        },
    )
    .await?;

    let suggestions: Vec<String> = list
        .iter()
        .filter(|m| m.content.to_lowercase().contains(&query.q.to_lowercase()))
        .take(limit)
        .map(|m| m.content.clone())
        .collect();

    Ok(Json(json!({ "query": query.q, "suggestions": suggestions })))
}
