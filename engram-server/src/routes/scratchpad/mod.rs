use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, post},
    Json, Router,
};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;

mod types;
use types::{PromoteBody, ScratchQuery};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/scratch", get(list_scratch).put(put_scratch))
        .route("/scratch/{session}", delete(delete_session))
        .route("/scratch/{session}/{key}", delete(delete_key))
        .route("/scratch/{session}/promote", post(promote))
}

async fn list_scratch(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Query(q): Query<ScratchQuery>,
) -> Result<Json<Value>, AppError> {
    let entries = kleos_lib::scratchpad::list_entries(
        &state.db,
        auth.user_id,
        q.agent.as_deref(),
        q.model.as_deref(),
        q.session.as_deref(),
    )
    .await?;
    let count = entries.len();
    Ok(Json(json!({ "entries": entries, "count": count })))
}

async fn put_scratch(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Json(body): Json<kleos_lib::scratchpad::ScratchPutBody>,
) -> Result<Json<Value>, AppError> {
    let session = body.session.as_deref().unwrap_or("default");
    let agent = body.agent.as_deref().unwrap_or("unknown");
    let model = body.model.as_deref().unwrap_or("");
    let ttl = body.ttl.unwrap_or(30).clamp(1, 1440);
    let entries = body.entries.unwrap_or_default();
    let mut stored = 0;
    for e in &entries {
        let value = e.value.as_deref().unwrap_or("");
        kleos_lib::scratchpad::upsert_entry(
            &state.db,
            auth.user_id,
            session,
            agent,
            model,
            &e.key,
            value,
            ttl,
        )
        .await?;
        stored += 1;
    }
    Ok(Json(
        json!({ "stored": stored, "session": session, "ttl_minutes": ttl }),
    ))
}

async fn delete_session(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::scratchpad::delete_session(&state.db, auth.user_id, &session).await?;
    Ok(Json(json!({ "deleted": true, "session": session })))
}

async fn delete_key(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path((session, key)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    kleos_lib::scratchpad::delete_session_key(&state.db, auth.user_id, &session, &key).await?;
    Ok(Json(
        json!({ "deleted": true, "session": session, "key": key }),
    ))
}

async fn promote(
    Auth(auth): Auth,
    State(state): State<AppState>,
    Path(session): Path<String>,
    Json(body): Json<PromoteBody>,
) -> Result<Json<Value>, AppError> {
    let combine = body.combine.unwrap_or(false);
    let category = body.category.as_deref().unwrap_or("discovery");
    let ids = kleos_lib::scratchpad::promote_entries(
        &state.db,
        auth.user_id,
        &session,
        body.keys.as_deref(),
        combine,
        category,
    )
    .await?;
    Ok(Json(
        json!({ "promoted": true, "memory_ids": ids, "count": ids.len() }),
    ))
}
