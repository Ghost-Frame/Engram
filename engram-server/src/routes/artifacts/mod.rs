use axum::{
    extract::{Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use kleos_lib::artifacts::{self, StoreArtifactOpts};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};
use kleos_lib::validation::MAX_ARTIFACT_UPLOAD_BYTES as MAX_UPLOAD_BYTES;

#[allow(dead_code)]
mod types;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/artifacts/stats", get(get_stats))
        .route(
            "/artifacts/{memory_id}",
            get(list_for_memory).post(upload_artifact),
        )
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
    let owner: i64 = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id FROM memories WHERE id = ?1",
                rusqlite::params![memory_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Not found".into())))?;
    if owner != auth.user_id {
        return Err(AppError(kleos_lib::EngError::NotFound("Not found".into())));
    }

    let artifacts = artifacts::get_artifacts_by_memory(&state.db, memory_id, auth.user_id).await?;
    Ok(Json(
        json!({ "artifacts": artifacts, "memory_id": memory_id }),
    ))
}

/// Upload an artifact attached to a memory.
///
/// Accepts multipart/form-data with:
///   - `file` (required): the file data
///   - `name` (optional): display name, defaults to filename
///   - `artifact_type` (optional): defaults to "file"
///   - `source_url` (optional)
///   - `agent` (optional)
///   - `session_id` (optional)
///   - `metadata` (optional): JSON string
async fn upload_artifact(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(memory_id): Path<i64>,
    mut multipart: Multipart,
) -> Result<Json<Value>, AppError> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut file_mime: Option<String> = None;
    let mut name: Option<String> = None;
    let mut artifact_type: Option<String> = None;
    let mut source_url: Option<String> = None;
    let mut agent: Option<String> = None;
    let mut session_id: Option<String> = None;
    let mut metadata: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                file_mime = field.content_type().map(|s| s.to_string());
                file_name = field.file_name().map(|s| s.to_string());
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?;
                if bytes.len() > MAX_UPLOAD_BYTES {
                    return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                        "File too large: {} bytes (max {})",
                        bytes.len(),
                        MAX_UPLOAD_BYTES
                    ))));
                }
                file_data = Some(bytes.to_vec());
            }
            "name" => {
                name =
                    Some(field.text().await.map_err(|e| {
                        AppError(kleos_lib::EngError::InvalidInput(e.to_string()))
                    })?);
            }
            "artifact_type" => {
                artifact_type =
                    Some(field.text().await.map_err(|e| {
                        AppError(kleos_lib::EngError::InvalidInput(e.to_string()))
                    })?);
            }
            "source_url" => {
                source_url =
                    Some(field.text().await.map_err(|e| {
                        AppError(kleos_lib::EngError::InvalidInput(e.to_string()))
                    })?);
            }
            "agent" => {
                agent =
                    Some(field.text().await.map_err(|e| {
                        AppError(kleos_lib::EngError::InvalidInput(e.to_string()))
                    })?);
            }
            "session_id" => {
                session_id =
                    Some(field.text().await.map_err(|e| {
                        AppError(kleos_lib::EngError::InvalidInput(e.to_string()))
                    })?);
            }
            "metadata" => {
                metadata =
                    Some(field.text().await.map_err(|e| {
                        AppError(kleos_lib::EngError::InvalidInput(e.to_string()))
                    })?);
            }
            _ => {
                // skip unknown fields
            }
        }
    }

    let data = file_data.ok_or_else(|| {
        AppError(kleos_lib::EngError::InvalidInput(
            "missing required 'file' field".into(),
        ))
    })?;

    let filename = file_name.unwrap_or_else(|| "unnamed".to_string());
    let mime_type = file_mime.unwrap_or_else(|| "application/octet-stream".to_string());
    let display_name = name.unwrap_or_else(|| filename.clone());
    let size_bytes = data.len() as i64;
    let sha256 = artifacts::sha256_hex(&data);

    // Extract text content for indexable types
    let content = if artifacts::is_indexable_mime_type(&mime_type) {
        std::str::from_utf8(&data).ok().map(|s| s.to_string())
    } else {
        None
    };

    let opts = StoreArtifactOpts {
        artifact_type,
        content,
        source_url,
        agent,
        session_id,
        metadata,
    };

    let artifact_id = artifacts::store_artifact(
        &state.db,
        auth.user_id,
        memory_id,
        &display_name,
        &filename,
        &mime_type,
        size_bytes,
        &sha256,
        "inline",
        Some(data.clone()),
        None,
        false,
        &opts,
    )
    .await?;

    // Index for FTS if applicable
    artifacts::index_artifact(&state.db, artifact_id, auth.user_id, &mime_type, &data).await;

    Ok(Json(json!({
        "id": artifact_id,
        "memory_id": memory_id,
        "filename": filename,
        "mime_type": mime_type,
        "size_bytes": size_bytes,
        "sha256": sha256,
    })))
}

async fn download_artifact(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let artifact = artifacts::get_artifact_by_id(&state.db, id, auth.user_id)
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Artifact not found".into())))?;

    // Verify the owning memory belongs to this user
    // Reject orphaned artifacts (no memory_id) to prevent BOLA
    let memory_id = artifact.memory_id.ok_or_else(|| {
        AppError(kleos_lib::EngError::NotFound(
            "Artifact has no associated memory".into(),
        ))
    })?;

    let owner: i64 = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id FROM memories WHERE id = ?1",
                rusqlite::params![memory_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Not found".into())))?;
    if owner != auth.user_id {
        return Err(AppError(kleos_lib::EngError::NotFound("Not found".into())));
    }

    // Get artifact data (inline storage only for now)
    let data = artifacts::get_artifact_data(&state.db, id, auth.user_id)
        .await?
        .ok_or_else(|| {
            AppError(kleos_lib::EngError::Internal(
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
