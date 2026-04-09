use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::apikeys;
use engram_lib::EngError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth-keys", get(list_keys_handler).post(create_key_handler))
        .route("/auth-keys/{id}", delete(revoke_key_handler))
        .route("/auth-keys/{id}/revoke", post(revoke_key_post_handler))
        .route("/auth-keys/{id}/rotate", post(rotate_key_handler))
}

#[derive(Debug, Deserialize)]
struct CreateKeyBody {
    scopes: Option<String>,
    rate_limit: Option<i64>,
}

async fn create_key_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateKeyBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let scopes = body.scopes.as_deref().unwrap_or("*");
    let rate_limit = body.rate_limit.unwrap_or(1000);
    let (key, full_key) = apikeys::create_api_key(&state.db, auth.user_id, scopes, rate_limit).await?;
    Ok((StatusCode::CREATED, Json(json!({ "key": key, "full_key": full_key }))))
}

async fn list_keys_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let keys = apikeys::list_api_keys(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "keys": keys, "count": keys.len() })))
}

async fn revoke_key_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    apikeys::delete_api_key(&state.db, id).await?;
    Ok(Json(json!({ "revoked": true, "id": id })))
}

async fn revoke_key_post_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    apikeys::delete_api_key(&state.db, id).await?;
    Ok(Json(json!({ "revoked": true, "id": id })))
}

async fn rotate_key_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT scopes, rate_limit FROM api_keys WHERE id = ?1 AND user_id = ?2 LIMIT 1",
            libsql::params![id, auth.user_id],
        )
        .await
        .map_err(EngError::Database)?;
    let row = rows
        .next()
        .await
        .map_err(EngError::Database)?
        .ok_or_else(|| EngError::NotFound("key not found".to_string()))?;
    let scopes: String = row.get(0).map_err(EngError::Database)?;
    let rate_limit: i64 = row.get(1).map_err(EngError::Database)?;

    apikeys::delete_api_key(&state.db, id).await?;
    let (new_key, full_key) =
        apikeys::create_api_key(&state.db, auth.user_id, &scopes, rate_limit).await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({ "rotated": true, "old_id": id, "key": new_key, "full_key": full_key })),
    ))
}
