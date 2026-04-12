use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use engram_lib::artifacts;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/artifacts/stats", get(get_stats))
        .route("/artifacts/{memory_id}", get(list_for_memory))
        .route("/artifact/{id}", get(download_artifact))
}

async fn get_stats(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
    let stats = artifacts::get_artifact_stats(&state.db, auth.user_id).await?;
    Ok(Json(json!({
        "total_count": stats.total_count,
        "total_bytes": stats.total_bytes,
        "inline": { "count": stats.inline_count, "bytes": stats.inline_bytes },
        "disk": { "count": stats.disk_count, "bytes": stats.disk_bytes },
    })))
}

async fn list_for_memory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    // Verify the memory belongs to this user
    let mut rows = state
        .db
        .conn
        .query(
            "SELECT user_id FROM memories WHERE id = ?1",
            libsql::params![memory_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let row = rows
        .next()
        .await
        .map_err(engram_lib::EngError::Database)?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Not found".into())))?;

    let owner: i64 = row
        .get(0)
        .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
    if owner != auth.user_id {
        return Err(AppError(engram_lib::EngError::NotFound("Not found".into())));
    }

    let artifacts = artifacts::get_artifacts_by_memory(&state.db, memory_id, auth.user_id).await?;
    Ok(Json(
        json!({ "artifacts": artifacts, "memory_id": memory_id }),
    ))
}

async fn download_artifact(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let artifact = artifacts::get_artifact_by_id(&state.db, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(engram_lib::EngError::NotFound("Artifact not found".into())))?;

    // Verify the owning memory belongs to this user
    // Reject orphaned artifacts (no memory_id) to prevent BOLA
    let memory_id = artifact.memory_id.ok_or_else(|| {
        AppError(engram_lib::EngError::NotFound(
            "Artifact has no associated memory".into(),
        ))
    })?;

    let mut rows = state
        .db
        .conn
        .query(
            "SELECT user_id FROM memories WHERE id = ?1",
            libsql::params![memory_id],
        )
        .await
        .map_err(engram_lib::EngError::Database)?;

    let row = rows.next().await.map_err(engram_lib::EngError::Database)?;

    match row {
        Some(r) => {
            let owner: i64 = r
                .get(0)
                .map_err(|e| engram_lib::EngError::Internal(e.to_string()))?;
            if owner != auth.user_id {
                return Err(AppError(engram_lib::EngError::NotFound("Not found".into())));
            }
        }
        None => {
            return Err(AppError(engram_lib::EngError::NotFound("Not found".into())));
        }
    }

    // Get artifact data (inline storage only for now)
    let data = artifacts::get_artifact_data(&state.db, id, auth.user_id)
        .await?
        .ok_or_else(|| {
            AppError(engram_lib::EngError::Internal(
                "Artifact has no data".into(),
            ))
        })?;

    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, artifact.mime_type.clone()),
            (header::CONTENT_LENGTH, data.len().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", artifact.filename),
            ),
        ],
        data,
    )
        .into_response())
}
