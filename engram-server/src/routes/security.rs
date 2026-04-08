use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use engram_lib::apikeys;
use engram_lib::audit;
use engram_lib::quota;
use engram_lib::ratelimit;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api-keys",
            post(create_api_key_handler).get(list_api_keys_handler),
        )
        .route("/api-keys/{id}", axum::routing::delete(delete_api_key_handler))
        .route("/audit", get(list_audit_handler))
        .route("/rate-limit/{key}", get(rate_limit_status_handler))
        .route("/quota", get(get_quota_handler))
        .route("/usage", post(record_usage_handler))
}

#[derive(Debug, Deserialize)]
struct CreateApiKeyBody {
    scopes: Option<String>,
    rate_limit: Option<i64>,
}

async fn create_api_key_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateApiKeyBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let scopes = body.scopes.as_deref().unwrap_or("*");
    let rate_limit = body.rate_limit.unwrap_or(1000);
    let (key_record, full_key) =
        apikeys::create_api_key(&state.db, auth.user_id, scopes, rate_limit).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "key": key_record, "full_key": full_key })),
    ))
}

async fn list_api_keys_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let keys = apikeys::list_api_keys(&state.db, auth.user_id).await?;
    Ok(Json(json!({ "keys": keys })))
}

async fn delete_api_key_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    apikeys::delete_api_key(&state.db, id).await?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

#[derive(Debug, Deserialize)]
struct AuditParams {
    limit: Option<usize>,
    target_type: Option<String>,
    target_id: Option<String>,
}

async fn list_audit_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(params): Query<AuditParams>,
) -> Result<Json<Value>, AppError> {
    let limit = params.limit.unwrap_or(50);
    let entries = audit::query_audit_log(
        &state.db,
        params.target_type.as_deref(),
        params.target_id.as_deref(),
        limit,
    )
    .await?;
    Ok(Json(json!({ "entries": entries, "count": entries.len() })))
}

async fn rate_limit_status_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(key): Path<String>,
) -> Result<Json<Value>, AppError> {
    let allowed = ratelimit::check_rate_limit(&state.db, &key, 1000, 60).await?;
    Ok(Json(json!({ "key": key, "allowed": allowed })))
}

async fn get_quota_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let status = quota::check_quota(&state.db, auth.user_id).await?;
    Ok(Json(json!(status)))
}

#[derive(Debug, Deserialize)]
struct RecordUsageBody {
    event_type: String,
    quantity: Option<i64>,
    agent_id: Option<i64>,
}

async fn record_usage_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<RecordUsageBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let quantity = body.quantity.unwrap_or(1);
    quota::record_usage(&state.db, auth.user_id, body.agent_id, &body.event_type, quantity)
        .await?;
    Ok((StatusCode::CREATED, Json(json!({ "recorded": true }))))
}
