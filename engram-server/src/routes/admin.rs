use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::auth::{create_key, list_keys, Scope};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bootstrap", post(bootstrap))
        .route("/keys", post(create_api_key).get(list_api_keys))
        .route("/stats", get(get_stats))
}

#[derive(Debug, Deserialize)]
struct CreateKeyRequest {
    name: String,
    scopes: Option<Vec<String>>,
    user_id: Option<i64>,
}

async fn count_rows(state: &AppState, sql: &str) -> Result<i64, AppError> {
    let mut rows = state
        .db
        .conn
        .query(sql, ())
        .await
        .map_err(engram_lib::EngError::Database)?;
    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| engram_lib::EngError::Internal("missing stats row".into()))?;
    row.get(0)
        .map_err(engram_lib::EngError::Database)
        .map_err(AppError)
}

async fn bootstrap(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let existing_count = count_rows(&state, "SELECT COUNT(*) FROM api_keys WHERE is_active = 1").await?;

    if existing_count > 0 {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "bootstrap already complete" })),
        ));
    }

    let scopes = vec![Scope::Read, Scope::Write, Scope::Admin];
    let (key, raw_key) = create_key(&state.db, 1, "admin", scopes).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "key": raw_key.clone(),
            "api_key": raw_key,
            "name": key.name,
            "scopes": key.scopes,
            "user_id": key.user_id,
            "message": "Bootstrap complete. Store this key -- it will not be shown again."
        })),
    ))
}

async fn create_api_key(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateKeyRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    // Require admin scope
    if !auth.has_scope(&Scope::Admin) {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin scope required" })),
        ));
    }

    let user_id = body.user_id.unwrap_or(auth.user_id);

    let scopes: Vec<Scope> = body
        .scopes
        .unwrap_or_else(|| vec!["write".to_string()])
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();

    let (key, raw_key) = create_key(&state.db, user_id, &body.name, scopes).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "key": raw_key,
            "id": key.id,
            "name": key.name,
            "scopes": key.scopes,
            "user_id": key.user_id,
            "created_at": key.created_at
        })),
    ))
}

async fn list_api_keys(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth(
            "admin scope required".to_string(),
        )));
    }

    let keys = list_keys(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "keys": keys })))
}

async fn get_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    if !auth.has_scope(&Scope::Admin) {
        return Err(AppError(engram_lib::EngError::Auth(
            "admin scope required".to_string(),
        )));
    }

    Ok(Json(json!({
        "memories": count_rows(&state, "SELECT COUNT(*) FROM memories").await?,
        "tasks": count_rows(&state, "SELECT COUNT(*) FROM tasks").await?,
        "events": count_rows(&state, "SELECT COUNT(*) FROM events").await?,
        "actions": count_rows(&state, "SELECT COUNT(*) FROM action_log").await?,
        "agents": count_rows(&state, "SELECT COUNT(*) FROM agents").await?,
        "api_keys": count_rows(&state, "SELECT COUNT(*) FROM api_keys WHERE is_active = 1").await?,
    })))
}
