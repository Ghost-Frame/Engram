use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use kleos_lib::artifacts::{self, StoreArtifactOpts};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use tokio_util::io::ReaderStream;

use crate::{
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};
use kleos_lib::validation::{
    ARTIFACT_DISK_TIER_THRESHOLD, MAX_ARTIFACT_UPLOAD_BYTES as MAX_UPLOAD_BYTES,
};

mod types;

/// Build the artifact route tree.
/// Overrides the global 2 MiB body limit with the advertised 50 MiB for uploads.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/artifacts/stats", get(get_stats))
        .route("/artifacts/search", post(search_artifacts_handler))
        .route(
            "/artifacts/{memory_id}",
            get(list_for_memory).post(upload_artifact),
        )
        .route(
            "/artifact/{id}",
            get(download_artifact).delete(delete_artifact_handler),
        )
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
}

/// Return aggregate artifact storage statistics (count and byte totals by storage tier).
async fn get_stats(ResolvedDb(db): ResolvedDb, Auth(auth): Auth) -> Result<Json<Value>, AppError> {
    let stats = artifacts::get_artifact_stats(&db, auth.effective_user_id()).await?;
    Ok(Json(json!({
        "total_count": stats.total_count,
        "total_bytes": stats.total_bytes,
        "inline": { "count": stats.inline_count, "bytes": stats.inline_bytes },
        "disk": { "count": stats.disk_count, "bytes": stats.disk_bytes },
    })))
}

/// List all artifacts attached to the given memory ID after verifying it exists.
async fn list_for_memory(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(memory_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let user_id = auth.effective_user_id();
    // Verify the memory exists in this tenant DB and belongs to the caller.
    // The user_id predicate is a no-op in a single-owner shard and the tenant
    // boundary in shared (monolith) mode.
    db.read(move |conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![memory_id, user_id],
                |_| Ok(()),
            )
            .optional()?)
    })
    .await?
    .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Memory not found".into())))?;

    let artifacts = artifacts::get_artifacts_by_memory(&db, user_id, memory_id).await?;
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
    ResolvedDb(db): ResolvedDb,
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
                name = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?,
                );
            }
            "artifact_type" => {
                artifact_type = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?,
                );
            }
            "source_url" => {
                source_url = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?,
                );
            }
            "agent" => {
                agent = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?,
                );
            }
            "session_id" => {
                session_id = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?,
                );
            }
            "metadata" => {
                metadata = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError(kleos_lib::EngError::InvalidInput(e.to_string())))?,
                );
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

    // Enforce per-tenant storage quota before writing.
    kleos_lib::quota::enforce_storage_quota(&db, size_bytes).await?;

    // Compute hash and extract indexable text from plaintext BEFORE encryption.
    let sha256 = artifacts::sha256_hex(&data);
    let content = artifacts::extract_indexable_content(&mime_type, &data);

    // Encrypt the data blob if artifact encryption is enabled.
    // The content column (plaintext for FTS) stays unencrypted -- accepted
    // trade-off per T12 in the design doc.
    let enc = &state.artifact_encryption;
    let (store_data, is_encrypted) = if enc.is_enabled() {
        let tenant_id = auth.effective_user_id().to_string();
        let encrypted = enc.encrypt_for_tenant(&tenant_id, &data)?;
        (encrypted, true)
    } else {
        (data, false)
    };

    let opts = StoreArtifactOpts {
        artifact_type,
        content,
        source_url,
        agent,
        session_id,
        metadata,
    };

    // Determine storage tier: > 1 MiB goes to disk.
    let (storage_mode, store_data_for_db, disk_path_str) =
        if store_data.len() > ARTIFACT_DISK_TIER_THRESHOLD {
            let blobs_dir = std::path::Path::new(db.db_path())
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join("blobs");
            let dest = artifacts::blob_path(&blobs_dir, &sha256, is_encrypted);

            // Create sharded subdirectory.
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    AppError(kleos_lib::EngError::Internal(format!(
                        "failed to create blob directory: {e}"
                    )))
                })?;
            }

            // Atomic write: tempfile in blobs_dir -> fsync -> persist.
            // If the DB insert fails later, the tempfile auto-drops (no orphan).
            let tmp = tempfile::NamedTempFile::new_in(&blobs_dir).map_err(|e| {
                AppError(kleos_lib::EngError::Internal(format!(
                    "failed to create temp file: {e}"
                )))
            })?;
            std::io::Write::write_all(&mut tmp.as_file(), &store_data).map_err(|e| {
                AppError(kleos_lib::EngError::Internal(format!(
                    "failed to write blob: {e}"
                )))
            })?;
            tmp.as_file().sync_all().map_err(|e| {
                AppError(kleos_lib::EngError::Internal(format!(
                    "failed to fsync blob: {e}"
                )))
            })?;

            let dest_str = dest.to_string_lossy().into_owned();

            // DB insert first -- if it fails, tmp auto-drops, no orphan file.
            let artifact_id = artifacts::store_artifact(
                &db,
                auth.effective_user_id(),
                memory_id,
                &display_name,
                &filename,
                &mime_type,
                size_bytes,
                &sha256,
                "disk",
                None,
                Some(&dest_str),
                is_encrypted,
                &opts,
            )
            .await?;

            // Persist after successful DB insert.
            tmp.persist(&dest).map_err(|e| {
                AppError(kleos_lib::EngError::Internal(format!(
                    "failed to persist blob: {e}"
                )))
            })?;

            return Ok(Json(json!({
                "id": artifact_id,
                "memory_id": memory_id,
                "filename": filename,
                "mime_type": mime_type,
                "size_bytes": size_bytes,
                "sha256": sha256,
                "storage_mode": "disk",
            })));
        } else {
            ("inline", Some(store_data), None::<String>)
        };

    let artifact_id = artifacts::store_artifact(
        &db,
        auth.effective_user_id(),
        memory_id,
        &display_name,
        &filename,
        &mime_type,
        size_bytes,
        &sha256,
        storage_mode,
        store_data_for_db,
        disk_path_str.as_deref(),
        is_encrypted,
        &opts,
    )
    .await?;

    Ok(Json(json!({
        "id": artifact_id,
        "memory_id": memory_id,
        "filename": filename,
        "mime_type": mime_type,
        "size_bytes": size_bytes,
        "sha256": sha256,
        "storage_mode": "inline",
    })))
}

/// Stream an artifact's binary data as an attachment, verifying its owning memory exists.
async fn download_artifact(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let user_id = auth.effective_user_id();
    let artifact = artifacts::get_artifact_by_id(&db, user_id, id)
        .await?
        .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Artifact not found".into())))?;

    // Reject orphaned artifacts (no memory_id)
    let memory_id = artifact.memory_id.ok_or_else(|| {
        AppError(kleos_lib::EngError::NotFound(
            "Artifact has no associated memory".into(),
        ))
    })?;

    // Verify the owning memory exists in this tenant DB and belongs to the
    // caller (no-op in a single-owner shard, tenant boundary in monolith mode).
    db.read(move |conn| {
        Ok(conn
            .query_row(
                "SELECT 1 FROM memories WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![memory_id, user_id],
                |_| Ok(()),
            )
            .optional()?)
    })
    .await?
    .ok_or_else(|| AppError(kleos_lib::EngError::NotFound("Memory not found".into())))?;

    // Sanitize the filename to prevent Content-Disposition header injection.
    // Only alphanumeric characters, dots, hyphens, and underscores are
    // permitted; any other byte is stripped. Length is capped at 255.
    let safe_filename: String = artifact
        .filename
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .take(255)
        .collect();
    let safe_filename = if safe_filename.is_empty() {
        "download".to_string()
    } else {
        safe_filename
    };

    if artifact.storage_mode == "disk" {
        let disk_path = artifact.disk_path.as_deref().ok_or_else(|| {
            AppError(kleos_lib::EngError::Internal(
                "disk-tier artifact has no disk_path".into(),
            ))
        })?;

        if artifact.is_encrypted {
            // Encrypted disk artifacts must be read fully to decrypt.
            let raw_data = tokio::fs::read(disk_path).await.map_err(|e| {
                AppError(kleos_lib::EngError::Internal(format!(
                    "failed to read disk blob: {e}"
                )))
            })?;
            let tenant_id = auth.effective_user_id().to_string();
            let data = state
                .artifact_encryption
                .decrypt_for_tenant(&tenant_id, &raw_data)?;

            Ok((
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, artifact.mime_type.clone()),
                    (header::CONTENT_LENGTH, data.len().to_string()),
                    (
                        header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{}\"", safe_filename),
                    ),
                ],
                data,
            )
                .into_response())
        } else {
            // Unencrypted disk artifacts can be streamed directly.
            let file = tokio::fs::File::open(disk_path).await.map_err(|e| {
                AppError(kleos_lib::EngError::Internal(format!(
                    "failed to open disk blob: {e}"
                )))
            })?;
            let stream = ReaderStream::new(file);
            let body = Body::from_stream(stream);

            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, &artifact.mime_type)
                .header(header::CONTENT_LENGTH, artifact.size_bytes.to_string())
                .header(
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", safe_filename),
                )
                .body(body)
                .unwrap()
                .into_response())
        }
    } else {
        // Inline storage -- read from DB.
        let raw_data = artifacts::get_artifact_data(&db, user_id, id)
            .await?
            .ok_or_else(|| {
                AppError(kleos_lib::EngError::Internal("Artifact has no data".into()))
            })?;

        let data = if artifact.is_encrypted {
            let tenant_id = auth.effective_user_id().to_string();
            state
                .artifact_encryption
                .decrypt_for_tenant(&tenant_id, &raw_data)?
        } else {
            raw_data
        };

        Ok((
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, artifact.mime_type.clone()),
                (header::CONTENT_LENGTH, data.len().to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{}\"", safe_filename),
                ),
            ],
            data,
        )
            .into_response())
    }
}

/// DELETE /artifact/{id} -- permanently remove an artifact.
///
/// If the artifact lived on disk, unlinks the blob file after the DB row is
/// removed. The FTS cleanup happens automatically via the `artifacts_fts_delete`
/// trigger. Returns 204 No Content on success (idempotent -- 204 even when the
/// artifact did not exist).
async fn delete_artifact_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let disk_path = artifacts::delete_artifact(&db, auth.effective_user_id(), id).await?;

    if let Some(path) = disk_path {
        if let Err(e) = tokio::fs::remove_file(&path).await {
            tracing::warn!(artifact_id = id, path = %path, "failed to unlink disk blob: {e}");
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /artifacts/search -- full-text search across artifact name and content.
///
/// Uses the `artifacts_fts` FTS5 virtual table. Results are ordered by BM25
/// rank (best match first). The limit is capped at 100 to prevent unbounded
/// result sets.
async fn search_artifacts_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Json(body): Json<types::ArtifactSearchBody>,
) -> Result<Json<Value>, AppError> {
    let limit = body.limit.map(|l| l.min(100)).unwrap_or(20);
    let results = artifacts::search_artifacts(
        &db,
        auth.effective_user_id(),
        &body.query,
        limit,
        body.memory_id,
    )
    .await?;
    let total = results.len();
    Ok(Json(json!({ "results": results, "total": total })))
}
