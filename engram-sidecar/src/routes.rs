use axum::{
    extract::State,
    http::StatusCode,
    middleware,
    routing::{get, post},
    Json, Router,
};
use engram_lib::memory::{
    self,
    search::hybrid_search,
    types::{SearchRequest, StoreRequest},
};
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

use crate::auth::require_token;
use crate::session::Observation;
use crate::SidecarState;

const FLUSH_THRESHOLD: usize = 5;

pub fn router(state: SidecarState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/observe", post(observe))
        .route("/recall", post(recall))
        .route("/end", post(end_session))
        .layer(middleware::from_fn_with_state(state.clone(), require_token))
        .with_state(state)
}

async fn health(State(state): State<SidecarState>) -> Json<Value> {
    let session = state.session.read().await;
    Json(json!({
        "status": "ok",
        "session_id": session.id,
        "observation_count": session.observation_count,
        "stored_count": session.stored_count,
        "ended": session.ended,
    }))
}

#[derive(Debug, Deserialize)]
struct ObserveBody {
    pub tool_name: String,
    pub content: String,
    #[serde(default = "default_importance")]
    pub importance: i32,
    #[serde(default = "default_category")]
    pub category: String,
}

fn default_importance() -> i32 {
    3
}

fn default_category() -> String {
    "discovery".to_string()
}

async fn observe(
    State(state): State<SidecarState>,
    Json(body): Json<ObserveBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let obs = Observation {
        tool_name: body.tool_name,
        content: body.content,
        importance: body.importance,
        category: body.category,
        timestamp: chrono::Utc::now(),
    };

    let pending_count = {
        let mut session = state.session.write().await;
        if session.ended {
            return Err((
                StatusCode::GONE,
                Json(json!({ "error": "session has ended" })),
            ));
        }
        session.add_observation(obs)
    };

    let flushed = if pending_count >= FLUSH_THRESHOLD {
        flush_pending(&state).await
    } else {
        0
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "accepted": true,
            "pending": pending_count.saturating_sub(flushed),
            "flushed": flushed,
        })),
    ))
}

#[derive(Debug, Deserialize)]
struct RecallBody {
    pub query: String,
    #[serde(default = "default_recall_limit")]
    pub limit: usize,
}

fn default_recall_limit() -> usize {
    10
}

async fn recall(
    State(state): State<SidecarState>,
    Json(body): Json<RecallBody>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let embedding = if let Some(ref embedder) = state.embedder {
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

    let req = SearchRequest {
        query: body.query,
        embedding,
        limit: Some(body.limit),
        category: None,
        source: None,
        tags: None,
        threshold: None,
        user_id: Some(state.user_id),
        space_id: None,
        include_forgotten: Some(false),
        mode: None,
        question_type: None,
        expand_relationships: false,
        include_links: false,
        latest_only: true,
        source_filter: None,
    };

    // SECURITY (SEC-MED-6): do not leak libsql table/column names or the
    // inner error string to unauthenticated callers. Log server-side only.
    let results = hybrid_search(&state.db, req).await.map_err(|e| {
        tracing::error!(error = %e, "sidecar hybrid_search failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "internal error" })),
        )
    })?;

    let session_id = {
        let session = state.session.read().await;
        session.id.clone()
    };

    Ok(Json(json!({
        "results": results,
        "count": results.len(),
        "session_id": session_id,
    })))
}

async fn end_session(
    State(state): State<SidecarState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let flushed = flush_pending(&state).await;

    let mut session = state.session.write().await;
    session.end();

    let duration = chrono::Utc::now()
        .signed_duration_since(session.started_at)
        .num_seconds();

    info!(
        session_id = %session.id,
        observations = session.observation_count,
        stored = session.stored_count,
        duration_secs = duration,
        "session ended"
    );

    Ok(Json(json!({
        "ended": true,
        "session_id": session.id,
        "flushed": flushed,
        "observation_count": session.observation_count,
        "stored_count": session.stored_count,
        "duration_secs": duration,
    })))
}

async fn flush_pending(state: &SidecarState) -> usize {
    let observations = {
        let mut session = state.session.write().await;
        session.drain_pending()
    };

    if observations.is_empty() {
        return 0;
    }

    let session_id = {
        let session = state.session.read().await;
        session.id.clone()
    };

    let mut stored = 0usize;
    for obs in &observations {
        let embedding = if let Some(ref embedder) = state.embedder {
            match embedder.embed(&obs.content).await {
                Ok(emb) => Some(emb),
                Err(e) => {
                    tracing::warn!("embedding failed for observation: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let req = StoreRequest {
            content: format!("[{}] {}", obs.tool_name, obs.content),
            category: obs.category.clone(),
            source: state.source.clone(),
            importance: obs.importance,
            tags: Some(vec!["sidecar".to_string(), obs.tool_name.clone()]),
            embedding,
            session_id: Some(session_id.clone()),
            is_static: None,
            user_id: Some(state.user_id),
            space_id: None,
            parent_memory_id: None,
        };

        match memory::store(&state.db, req).await {
            Ok(result) => {
                stored += 1;
                if result.created {
                    tracing::debug!(id = result.id, tool = %obs.tool_name, "observation stored");
                } else if let Some(dup) = result.duplicate_of {
                    tracing::debug!(dup_of = dup, tool = %obs.tool_name, "observation was duplicate");
                }
            }
            Err(e) => {
                tracing::error!(tool = %obs.tool_name, error = %e, "failed to store observation");
            }
        }
    }

    stored
}
