use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::AppError;
use crate::extractors::Auth;
use crate::state::AppState;
use engram_lib::artifacts;
use engram_lib::EngError;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/artifacts/store", post(store_artifact_handler))
        .route("/artifacts/stats", get(artifact_stats_handler))
        .route("/artifacts/search", get(search_artifacts_handler))
        .route("/artifacts/{id}", get(get_artifact_handler).delete(delete_artifact_handler))
        .route("/artifacts/{id}/data", get(get_artifact_data_handler))
        .route("/artifacts/{id}/metadata", patch(update_artifact_metadata_handler))
        .route("/artifacts/memory/{memory_id}", get(list_by_memory_handler))
}

#[derive(Debug, Deserialize)]
struct StoreArtifactBody {
    memory_id: i64,
    filename: String,
    mime_type: Option<String>,
    storage_mode: Option<String>,
    data: Option<String>,
    disk_path: Option<String>,
    is_encrypted: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct UpdateMetadataBody {
    filename: Option<String>,
    mime_type: Option<String>,
}

async fn store_artifact_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<StoreArtifactBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let mime = body
        .mime_type
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let storage_mode = body.storage_mode.unwrap_or_else(|| "inline".to_string());
    let payload = body.data.unwrap_or_default().into_bytes();
    let size = payload.len() as i64;
    let sha256 = artifacts::sha256_hex(&payload);
    let is_encrypted = body.is_encrypted.unwrap_or(false);

    let artifact_id = artifacts::store_artifact(
        &state.db,
        body.memory_id,
        &body.filename,
        &mime,
        size,
        &sha256,
        &storage_mode,
        Some(payload.clone()),
        body.disk_path.as_deref(),
        is_encrypted,
    )
    .await?;

    let indexed = artifacts::index_artifact(&state.db, artifact_id, &mime, &payload).await;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": artifact_id,
            "memory_id": body.memory_id,
            "filename": body.filename,
            "mime_type": mime,
            "size_bytes": size,
            "sha256": sha256,
            "indexed": indexed,
        })),
    ))
}

async fn list_by_memory_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let rows = artifacts::get_artifacts_by_memory(&state.db, memory_id).await?;
    Ok(Json(json!({ "artifacts": rows, "count": rows.len() })))
}

async fn get_artifact_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let row = artifacts::get_artifact_by_id(&state.db, id).await?;
    Ok(Json(json!({ "artifact": row })))
}

async fn get_artifact_data_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let data = artifacts::get_artifact_data(&state.db, id).await?;
    let body = data
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default();
    Ok(Json(json!({ "id": id, "data": body })))
}

async fn search_artifacts_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = query.limit.unwrap_or(20);
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT a.id, COALESCE(a.filename, a.name), a.mime_type, a.size_bytes, a.created_at
             FROM artifacts_fts f
             JOIN artifacts a ON a.id = f.rowid
             WHERE artifacts_fts MATCH ?1
             LIMIT ?2",
            libsql::params![query.q, limit],
        )
        .await
        .map_err(EngError::Database)?;
    let mut results: Vec<Value> = Vec::new();
    while let Some(row) = rows.next().await.map_err(EngError::Database)? {
        results.push(json!({
            "id": row.get::<i64>(0).map_err(EngError::Database)?,
            "filename": row.get::<String>(1).map_err(EngError::Database)?,
            "mime_type": row.get::<Option<String>>(2).map_err(EngError::Database)?,
            "size_bytes": row.get::<Option<i64>>(3).map_err(EngError::Database)?,
            "created_at": row.get::<Option<String>>(4).map_err(EngError::Database)?,
        }));
    }
    Ok(Json(json!({ "results": results })))
}

async fn update_artifact_metadata_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
    Json(body): Json<UpdateMetadataBody>,
) -> Result<Json<Value>, AppError> {
    state
        .db
        .conn
        .execute(
            "UPDATE artifacts
             SET filename = COALESCE(?1, filename),
                 name = COALESCE(?1, name),
                 mime_type = COALESCE(?2, mime_type),
                 updated_at = datetime('now')
             WHERE id = ?3",
            libsql::params![body.filename, body.mime_type, id],
        )
        .await
        .map_err(EngError::Database)?;
    Ok(Json(json!({ "updated": true, "id": id })))
}

async fn delete_artifact_handler(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Path(id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    state
        .db
        .conn
        .execute("DELETE FROM artifacts WHERE id = ?1", libsql::params![id])
        .await
        .map_err(EngError::Database)?;
    Ok(Json(json!({ "deleted": true, "id": id })))
}

async fn artifact_stats_handler(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = artifacts::get_artifact_stats(&state.db, Some(auth.user_id)).await?;
    Ok(Json(json!(stats)))
}
