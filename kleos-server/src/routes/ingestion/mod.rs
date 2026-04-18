// Ingestion routes -- ported from ingestion/routes.ts

mod types;

use axum::{
    extract::{DefaultBodyLimit, Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::Engine as _;
use kleos_lib::ingestion::{
    self,
    types::{FormatMeta, IngestMode, IngestOptions, IngestProgressEvent, SupportedFormat},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::convert::Infallible;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use uuid::Uuid;

use rusqlite::{params, OptionalExtension};

use crate::{error::AppError, extractors::Auth, state::AppState};
use kleos_lib::validation::{
    MAX_IMPORT_BATCH, MAX_INGEST_TEXT_BYTES, MAX_UPLOAD_CHUNK_BYTES, MAX_UPLOAD_TOTAL_BYTES,
};
use types::{
    ImportBulkBody, ImportJsonBody, IngestBody, UploadAbortBody, UploadChunkBody,
    UploadCompleteBody, UploadInitBody, UploadSession,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/import/bulk", post(import_bulk))
        .route("/import/json", post(import_json))
        .route("/import/mem0", post(import_mem0))
        .route("/import/supermemory", post(import_supermemory))
        .route("/ingest", post(ingest_text))
        .route("/ingest/stream", post(ingest_text_stream))
        .route("/ingest/upload/init", post(upload_init))
        .route("/ingest/upload/chunk", post(upload_chunk))
        .route("/ingest/upload/complete", post(upload_complete))
        .route("/ingest/upload/abort", post(upload_abort))
        .route("/ingest/upload/{upload_id}/status", get(upload_status))
        .route("/add", post(add_conversation))
        .route("/derive", post(derive))
        // S7-26: ingestion may chunk + embed large documents; allow 120s.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(120),
        ))
        // S7-27: ingestion + chunk uploads allow up to 10 MiB per request.
        .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
}

// ---------------------------------------------------------------------------
// 3.8: Resumable / chunked ingest upload
//
// Flow:
//   1. client POSTs /ingest/upload/init -> server returns upload_id + expiry
//   2. client POSTs each chunk to /ingest/upload/chunk with (upload_id,
//      chunk_index, chunk_hash, data[base64]); server verifies hash, persists,
//      and idempotently ignores duplicate (upload_id, chunk_index) rows so a
//      retried chunk is a no-op.
//   3. client POSTs /ingest/upload/complete to finalize -- server reassembles
//      chunks 0..N-1, optionally verifies a final SHA256, feeds the text into
//      the normal ingest pipeline, then marks the session completed.
//   4. client may POST /ingest/upload/abort or query /ingest/upload/{id}/status
//      at any time to inspect / cancel the session.
// ---------------------------------------------------------------------------

/// Session lifetime before automatic expiry (24 hours).
const UPLOAD_SESSION_TTL_HOURS: i64 = 24;

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

async fn upload_init(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<UploadInitBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if let Some(size) = body.total_size {
        if !(0..=MAX_UPLOAD_TOTAL_BYTES).contains(&size) {
            return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                "total_size must be between 0 and {} bytes",
                MAX_UPLOAD_TOTAL_BYTES
            ))));
        }
    }
    let upload_id = Uuid::new_v4().to_string();
    let source = body.source.unwrap_or_else(|| "upload".to_string());
    let expires_at = (chrono::Utc::now() + chrono::Duration::hours(UPLOAD_SESSION_TTL_HOURS))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    let chunk_size = body.chunk_size.unwrap_or(MAX_UPLOAD_CHUNK_BYTES as i64);

    let upload_id_db = upload_id.clone();
    let expires_at_db = expires_at.clone();
    let user_id = auth.user_id;
    let filename = body.filename.clone();
    let content_type = body.content_type.clone();
    let source_db = source.clone();
    let total_size = body.total_size;
    let total_chunks = body.total_chunks;

    state
        .db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO upload_sessions
                   (upload_id, user_id, filename, content_type, source,
                    total_size, total_chunks, chunk_size, status, expires_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'active', ?9)",
                params![
                    upload_id_db,
                    user_id,
                    filename,
                    content_type,
                    source_db,
                    total_size,
                    total_chunks,
                    chunk_size,
                    expires_at_db
                ],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "upload_id": upload_id,
            "expires_at": expires_at,
            "chunk_size": chunk_size,
            "max_chunk_bytes": MAX_UPLOAD_CHUNK_BYTES,
            "max_total_bytes": MAX_UPLOAD_TOTAL_BYTES,
            "source": source,
        })),
    ))
}

async fn load_session(
    state: &AppState,
    upload_id: &str,
) -> Result<UploadSession, kleos_lib::EngError> {
    let id = upload_id.to_string();
    let row: Option<UploadSession> = state
        .db
        .read(move |conn| {
            conn.query_row(
                "SELECT user_id, status, source, filename, content_type,
                        total_chunks, total_size, expires_at
                   FROM upload_sessions WHERE upload_id = ?1",
                params![id],
                |row| {
                    Ok(UploadSession {
                        user_id: row.get(0)?,
                        status: row.get(1)?,
                        source: row.get(2)?,
                        filename: row.get(3)?,
                        content_type: row.get(4)?,
                        total_chunks: row.get(5)?,
                        total_size: row.get(6)?,
                        expires_at: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await?;
    row.ok_or_else(|| kleos_lib::EngError::NotFound("upload session not found".into()))
}

fn ensure_session_owner(session: &UploadSession, user_id: i64) -> Result<(), AppError> {
    if session.user_id != user_id {
        return Err(AppError(kleos_lib::EngError::Auth(
            "upload belongs to another user".into(),
        )));
    }
    if session.status != "active" {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "upload is {}",
            session.status
        ))));
    }
    let expires = chrono::NaiveDateTime::parse_from_str(&session.expires_at, "%Y-%m-%d %H:%M:%S")
        .map(|dt| dt.and_utc())
        .unwrap_or_else(|_| chrono::Utc::now() + chrono::Duration::hours(1));
    if chrono::Utc::now() > expires {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "upload session expired".into(),
        )));
    }
    Ok(())
}

async fn upload_chunk(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<UploadChunkBody>,
) -> Result<Json<Value>, AppError> {
    if body.chunk_index < 0 {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "chunk_index must be >= 0".into(),
        )));
    }

    let session = load_session(&state, &body.upload_id).await?;
    ensure_session_owner(&session, auth.user_id)?;

    let raw = base64::engine::general_purpose::STANDARD
        .decode(body.data.as_bytes())
        .map_err(|_| AppError(kleos_lib::EngError::InvalidInput("invalid base64".into())))?;
    if raw.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "chunk data is empty".into(),
        )));
    }
    if raw.len() > MAX_UPLOAD_CHUNK_BYTES {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "chunk exceeds {} bytes",
            MAX_UPLOAD_CHUNK_BYTES
        ))));
    }
    let computed = sha256_hex(&raw);
    if computed != body.chunk_hash.to_ascii_lowercase() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "chunk_hash mismatch".into(),
        )));
    }

    let upload_id = body.upload_id.clone();
    let chunk_index = body.chunk_index;
    let chunk_hash = body.chunk_hash.clone();
    let raw_for_db = raw.clone();
    let size = raw.len() as i64;

    let (total_received, total_bytes) = state
        .db
        .write(move |conn| {
            // Guard against exceeding the per-session disk cap even before
            // this chunk lands. We check the *current* aggregate (excluding
            // any prior write of the same (upload_id, chunk_index) since that
            // row is about to be overwritten) so a malicious client cannot
            // grow the session past MAX_UPLOAD_TOTAL_BYTES through retries.
            let existing_size: Option<i64> = conn
                .query_row(
                    "SELECT size FROM upload_chunks WHERE upload_id = ?1 AND chunk_index = ?2",
                    params![upload_id, chunk_index],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let projected_total: i64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(size), 0) FROM upload_chunks WHERE upload_id = ?1",
                    params![upload_id],
                    |row| row.get(0),
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let adjusted = projected_total - existing_size.unwrap_or(0) + size;
            if adjusted > MAX_UPLOAD_TOTAL_BYTES {
                return Err(kleos_lib::EngError::InvalidInput(format!(
                    "upload would exceed {} byte limit",
                    MAX_UPLOAD_TOTAL_BYTES
                )));
            }

            conn.execute(
                "INSERT INTO upload_chunks (upload_id, chunk_index, chunk_hash, size, data)
                   VALUES (?1, ?2, ?3, ?4, ?5)
                   ON CONFLICT(upload_id, chunk_index) DO UPDATE SET
                     chunk_hash = excluded.chunk_hash,
                     size = excluded.size,
                     data = excluded.data,
                     created_at = datetime('now')",
                params![upload_id, chunk_index, chunk_hash, size, raw_for_db],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;

            let (count, bytes): (i64, i64) = conn
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM upload_chunks WHERE upload_id = ?1",
                    params![upload_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok((count, bytes))
        })
        .await?;

    Ok(Json(json!({
        "upload_id": body.upload_id,
        "chunk_index": body.chunk_index,
        "received": true,
        "chunks_received": total_received,
        "bytes_received": total_bytes,
        "expected_chunks": session.total_chunks,
    })))
}

async fn upload_complete(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<UploadCompleteBody>,
) -> Result<Json<Value>, AppError> {
    let session = load_session(&state, &body.upload_id).await?;
    ensure_session_owner(&session, auth.user_id)?;

    let upload_id = body.upload_id.clone();
    let chunks: Vec<(i64, String, Vec<u8>)> = state
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT chunk_index, chunk_hash, data FROM upload_chunks
                       WHERE upload_id = ?1 ORDER BY chunk_index ASC",
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(params![upload_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                })
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?);
            }
            Ok(out)
        })
        .await?;

    if chunks.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "no chunks uploaded".into(),
        )));
    }

    // Verify chunks form a contiguous 0..N-1 run -- a gap means a chunk
    // silently dropped and we refuse to assemble a corrupt body.
    for (expected, actual) in chunks.iter().enumerate() {
        if expected as i64 != actual.0 {
            return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                "missing chunk at index {} (next present: {})",
                expected, actual.0
            ))));
        }
    }
    let total = chunks.len() as i64;
    if let Some(expected) = body.total_chunks.or(session.total_chunks) {
        if expected != total {
            return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                "expected {} chunks, have {}",
                expected, total
            ))));
        }
    }

    // Reassemble.
    let mut assembled: Vec<u8> =
        Vec::with_capacity(chunks.iter().map(|(_, _, d)| d.len()).sum::<usize>().max(1));
    for (_, _, data) in &chunks {
        assembled.extend_from_slice(data);
    }

    if assembled.len() as i64 > MAX_UPLOAD_TOTAL_BYTES {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "assembled payload exceeds {} bytes",
            MAX_UPLOAD_TOTAL_BYTES
        ))));
    }

    let final_hash = sha256_hex(&assembled);
    if let Some(ref expected) = body.final_sha256 {
        if expected.to_ascii_lowercase() != final_hash {
            return Err(AppError(kleos_lib::EngError::InvalidInput(
                "final_sha256 mismatch -- payload corrupted".into(),
            )));
        }
    }

    // Text-only pipeline for now: treat the assembled bytes as UTF-8. Binary
    // formats can land here once ingestion grows binary-format support; for
    // now we reject non-UTF-8 cleanly rather than silently lossy-decoding.
    let text = String::from_utf8(assembled).map_err(|_| {
        AppError(kleos_lib::EngError::InvalidInput(
            "assembled payload is not valid UTF-8 -- binary ingest not yet supported".into(),
        ))
    })?;
    if text.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "assembled payload is empty".into(),
        )));
    }
    if text.len() > MAX_INGEST_TEXT_BYTES {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "assembled text exceeds {} bytes; decompose before upload",
            MAX_INGEST_TEXT_BYTES
        ))));
    }

    let mode = match body.mode.as_deref() {
        Some("raw") => IngestMode::Raw,
        _ => IngestMode::Extract,
    };
    let format: Option<SupportedFormat> = body
        .format
        .as_ref()
        .and_then(|f| f.parse().ok())
        .or_else(|| {
            session
                .filename
                .as_ref()
                .and_then(|f| f.rsplit_once('.').map(|(_, ext)| ext))
                .and_then(|ext| ext.parse().ok())
        });
    let options = IngestOptions {
        mode,
        format,
        source: session.source.clone(),
        category: body.category.unwrap_or_else(|| "general".to_string()),
        user_id: auth.user_id,
        space_id: None,
        project_id: body.project_id,
        episode_id: body.episode_id,
        entity_ids: None,
        chunker_options: None,
    };
    let meta = FormatMeta {
        extension: session
            .filename
            .as_ref()
            .and_then(|f| f.rsplit_once('.').map(|(_, ext)| ext.to_string())),
        mime: session.content_type.clone(),
    };

    let result = ingestion::ingest(&state.db, &text, options, Some(&meta)).await?;

    // Finalize: mark session complete and drop chunk blobs to reclaim space.
    let upload_id_db = body.upload_id.clone();
    let final_hash_db = final_hash.clone();
    state
        .db
        .write(move |conn| {
            conn.execute(
                "UPDATE upload_sessions SET status = 'completed',
                   completed_at = datetime('now'), final_sha256 = ?1
                   WHERE upload_id = ?2",
                params![final_hash_db, upload_id_db],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            conn.execute(
                "DELETE FROM upload_chunks WHERE upload_id = ?1",
                params![upload_id_db],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await?;

    Ok(Json(json!({
        "upload_id": body.upload_id,
        "status": "completed",
        "final_sha256": final_hash,
        "total_chunks": total,
        "total_bytes": text.len(),
        "job_id": result.job_id,
        "ingested_memories": result.total_memories,
        "chunks_processed": result.total_chunks,
        "errors": result.errors,
    })))
}

async fn upload_abort(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<UploadAbortBody>,
) -> Result<Json<Value>, AppError> {
    let session = load_session(&state, &body.upload_id).await?;
    if session.user_id != auth.user_id {
        return Err(AppError(kleos_lib::EngError::Auth(
            "upload belongs to another user".into(),
        )));
    }
    let upload_id = body.upload_id.clone();
    state
        .db
        .write(move |conn| {
            conn.execute(
                "UPDATE upload_sessions SET status = 'aborted',
                   completed_at = datetime('now') WHERE upload_id = ?1",
                params![upload_id],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            conn.execute(
                "DELETE FROM upload_chunks WHERE upload_id = ?1",
                params![upload_id],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        })
        .await?;

    Ok(Json(json!({
        "upload_id": body.upload_id,
        "status": "aborted",
    })))
}

async fn upload_status(
    State(state): State<AppState>,
    Auth(auth): Auth,
    AxumPath(upload_id): AxumPath<String>,
) -> Result<Json<Value>, AppError> {
    let session = load_session(&state, &upload_id).await?;
    if session.user_id != auth.user_id {
        return Err(AppError(kleos_lib::EngError::Auth(
            "upload belongs to another user".into(),
        )));
    }
    let id = upload_id.clone();
    let (chunks_received, bytes_received, received_indices): (i64, i64, Vec<i64>) = state
        .db
        .read(move |conn| {
            let (count, bytes): (i64, i64) = conn
                .query_row(
                    "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM upload_chunks WHERE upload_id = ?1",
                    params![id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let mut stmt = conn
                .prepare(
                    "SELECT chunk_index FROM upload_chunks WHERE upload_id = ?1 ORDER BY chunk_index",
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            let indices: Vec<i64> = stmt
                .query_map(params![id], |row| row.get::<_, i64>(0))
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();
            Ok((count, bytes, indices))
        })
        .await?;

    Ok(Json(json!({
        "upload_id": upload_id,
        "status": session.status,
        "filename": session.filename,
        "content_type": session.content_type,
        "source": session.source,
        "expires_at": session.expires_at,
        "expected_chunks": session.total_chunks,
        "expected_size": session.total_size,
        "chunks_received": chunks_received,
        "bytes_received": bytes_received,
        "received_indices": received_indices,
    })))
}

async fn import_bulk(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ImportBulkBody>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if body.text.is_none() && body.url.is_none() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "Provide text or url parameter".to_string(),
        )));
    }
    let input = if let Some(ref text) = body.text {
        if text.trim().is_empty() {
            return Err(AppError(kleos_lib::EngError::InvalidInput(
                "text must be a non-empty string".to_string(),
            )));
        }
        if text.len() > MAX_INGEST_TEXT_BYTES {
            return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                "text exceeds {} bytes; split the import",
                MAX_INGEST_TEXT_BYTES
            ))));
        }
        text.clone()
    } else {
        return Err(AppError(kleos_lib::EngError::NotImplemented(
            "URL fetching not yet implemented".to_string(),
        )));
    };
    let format: Option<SupportedFormat> = body.format.as_ref().and_then(|f| f.parse().ok());
    let mode = body
        .mode
        .as_ref()
        .map(|m| {
            if m == "extract" {
                IngestMode::Extract
            } else {
                IngestMode::Raw
            }
        })
        .unwrap_or(IngestMode::Extract);
    let options = IngestOptions {
        mode,
        format,
        source: body.source.unwrap_or_else(|| "import".to_string()),
        category: body.category.unwrap_or_else(|| "general".to_string()),
        user_id: auth.user_id,
        space_id: None,
        project_id: body.project_id,
        episode_id: body.episode_id,
        entity_ids: None,
        chunker_options: None,
    };
    let meta = FormatMeta {
        extension: None,
        mime: None,
    };
    let result = ingestion::ingest(&state.db, &input, options, Some(&meta)).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "job_id": result.job_id, "status": result.status,
            "total_documents": result.total_documents, "total_chunks": result.total_chunks,
            "total_memories": result.total_memories, "errors": result.errors,
            "duration_ms": result.duration_ms,
        })),
    ))
}

async fn import_json(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<ImportJsonBody>,
) -> Result<Json<Value>, AppError> {
    if body.version.is_none() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "Invalid export format: missing version field".to_string(),
        )));
    }
    let memories = body.memories.unwrap_or_default();
    if memories.is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "Invalid export format: missing memories array".to_string(),
        )));
    }
    if memories.len() > MAX_IMPORT_BATCH {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "import batch exceeds {} memories; split into smaller requests",
            MAX_IMPORT_BATCH
        ))));
    }
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for m in &memories {
        let content = match &m.content {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let tags_str = match &m.tags {
            Some(serde_json::Value::Array(arr)) => {
                Some(serde_json::to_string(arr).unwrap_or_default())
            }
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            _ => None,
        };
        let sync_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let created_at = m.created_at.clone().unwrap_or_else(|| now.clone());
        let updated_at = m.updated_at.clone().unwrap_or_else(|| now.clone());
        let category = m.category.clone().unwrap_or_else(|| "general".to_string());
        let source = m.source.clone().unwrap_or_else(|| "import".to_string());
        let session_id = m.session_id.clone();
        let importance = m.importance.unwrap_or(5);
        let confidence = m.confidence.unwrap_or(1.0);
        let is_static = if m.is_static.unwrap_or(false) {
            1i32
        } else {
            0i32
        };
        let user_id = auth.user_id;
        match state.db.write(move |conn| {
            conn.execute(
                "INSERT INTO memories (content, category, source, session_id, importance, tags, confidence, is_static, user_id, sync_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![content, category, source, session_id, importance, tags_str, confidence, is_static, user_id, sync_id, created_at, updated_at],
            ).map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        }).await {
            Ok(()) => imported += 1,
            Err(e) => { tracing::warn!("import_memory_failed: {}", e); skipped += 1; }
        }
    }
    Ok(Json(
        json!({ "imported": { "memories": imported, "skipped": skipped } }),
    ))
}

async fn import_mem0(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let memories = body
        .get("memories")
        .or_else(|| body.get("results"))
        .cloned()
        .unwrap_or(body.clone());
    let arr = memories.as_array().ok_or_else(|| {
        AppError(kleos_lib::EngError::InvalidInput(
            "Expected array of mem0 memories".to_string(),
        ))
    })?;
    if arr.len() > MAX_IMPORT_BATCH {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "import batch exceeds {} memories; split into smaller requests",
            MAX_IMPORT_BATCH
        ))));
    }
    let mut imported = 0i64;
    for mem in arr {
        let content = mem
            .get("memory")
            .or_else(|| mem.get("text"))
            .or_else(|| mem.get("content"))
            .and_then(|v| v.as_str());
        let content = match content {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => continue,
        };
        let meta_obj = mem.get("metadata").and_then(|m| m.as_object());
        let category = meta_obj
            .and_then(|m| m.get("category"))
            .and_then(|v| v.as_str())
            .or_else(|| mem.get("category").and_then(|v| v.as_str()))
            .unwrap_or("general");
        let source = meta_obj
            .and_then(|m| m.get("source"))
            .and_then(|v| v.as_str())
            .or_else(|| mem.get("source").and_then(|v| v.as_str()))
            .unwrap_or("mem0-import");
        let importance = meta_obj
            .and_then(|m| m.get("importance"))
            .and_then(|v| v.as_i64())
            .unwrap_or(5) as i32;
        let tags = meta_obj
            .and_then(|m| m.get("tags"))
            .cloned()
            .unwrap_or_else(|| json!(["mem0-import"]));
        let tags_str = serde_json::to_string(&tags).unwrap_or_default();
        let sync_id = Uuid::new_v4().to_string();
        let category_s = category.to_string();
        let source_s = source.to_string();
        let user_id = auth.user_id;
        if state.db.write(move |conn| {
            conn.execute(
                "INSERT INTO memories (content, category, source, importance, tags, confidence, user_id, sync_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6, ?7, datetime('now'), datetime('now'))",
                params![content, category_s, source_s, importance, tags_str, user_id, sync_id],
            ).map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        }).await.is_ok() {
            imported += 1;
        }
    }
    Ok(Json(json!({ "imported": imported, "source": "mem0" })))
}

async fn import_supermemory(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let items = body
        .get("documents")
        .or_else(|| body.get("memories"))
        .or_else(|| body.get("data"))
        .cloned()
        .or_else(|| {
            if body.is_array() {
                Some(body.clone())
            } else {
                None
            }
        })
        .ok_or_else(|| {
            AppError(kleos_lib::EngError::InvalidInput(
                "Expected documents/memories array".to_string(),
            ))
        })?;
    let arr = items.as_array().ok_or_else(|| {
        AppError(kleos_lib::EngError::InvalidInput(
            "Expected array".to_string(),
        ))
    })?;
    if arr.len() > MAX_IMPORT_BATCH {
        return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
            "import batch exceeds {} memories; split into smaller requests",
            MAX_IMPORT_BATCH
        ))));
    }
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for item in arr {
        let content = item
            .get("content")
            .or_else(|| item.get("text"))
            .or_else(|| item.get("description"))
            .or_else(|| item.get("raw"))
            .and_then(|v| v.as_str());
        let content = match content {
            Some(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => {
                skipped += 1;
                continue;
            }
        };
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let category = item
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| match item_type.to_lowercase().as_str() {
                "note" => "general",
                "tweet" | "page" | "bookmark" => "discovery",
                "document" => "task",
                "conversation" => "state",
                _ => "general",
            });
        let mut tags: Vec<String> = vec!["supermemory-import".to_string()];
        if let Some(spaces) = item.get("spaces").and_then(|v| v.as_array()) {
            for s in spaces {
                if let Some(sv) = s.as_str() {
                    tags.push(sv.to_lowercase());
                }
            }
        } else if let Some(space) = item.get("space").and_then(|v| v.as_str()) {
            tags.push(space.to_lowercase());
        }
        if let Some(item_tags) = item.get("tags").and_then(|v| v.as_array()) {
            for t in item_tags {
                if let Some(tv) = t.as_str() {
                    tags.push(tv.to_lowercase());
                }
            }
        }
        if !item_type.is_empty() {
            tags.push(item_type.to_lowercase());
        }
        tags.dedup();
        let tags_str = serde_json::to_string(&tags).unwrap_or_default();
        let importance = item
            .get("importance")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                item.get("metadata")
                    .and_then(|m| m.get("importance"))
                    .and_then(|v| v.as_i64())
            })
            .unwrap_or(5) as i32;
        let source = item
            .get("source")
            .and_then(|v| v.as_str())
            .or_else(|| {
                item.get("metadata")
                    .and_then(|m| m.get("source"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("supermemory-import");
        let sync_id = Uuid::new_v4().to_string();
        let category_s = category.to_string();
        let source_s = source.to_string();
        let user_id = auth.user_id;
        match state.db.write(move |conn| {
            conn.execute(
                "INSERT INTO memories (content, category, source, importance, tags, confidence, user_id, sync_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6, ?7, datetime('now'), datetime('now'))",
                params![content, category_s, source_s, importance, tags_str, user_id, sync_id],
            ).map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(())
        }).await {
            Ok(()) => imported += 1,
            Err(_) => { skipped += 1; }
        }
    }
    Ok(Json(
        json!({ "imported": imported, "skipped": skipped, "source": "supermemory" }),
    ))
}

async fn ingest_text(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<IngestBody>,
) -> Result<Json<Value>, AppError> {
    if body.url.is_none() && body.text.is_none() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "Provide url or text parameter".to_string(),
        )));
    }
    let raw_text;
    let ingest_source;
    let title;
    if let Some(ref text) = body.text {
        if text.trim().is_empty() {
            return Err(AppError(kleos_lib::EngError::InvalidInput(
                "text must be a non-empty string".to_string(),
            )));
        }
        if text.len() > MAX_INGEST_TEXT_BYTES {
            return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                "text exceeds {} bytes; split the ingest",
                MAX_INGEST_TEXT_BYTES
            ))));
        }
        raw_text = text.trim().to_string();
        title = body.title.unwrap_or_else(|| {
            let t: String = raw_text.chars().take(60).collect();
            t.replace(char::from(10u8), " ")
        });
        ingest_source = body.source.unwrap_or_else(|| "text".to_string());
    } else {
        return Err(AppError(kleos_lib::EngError::Internal(
            "URL fetching not yet implemented in Rust port".to_string(),
        )));
    }
    let options = IngestOptions {
        mode: IngestMode::Raw,
        format: None,
        source: ingest_source.clone(),
        category: "general".to_string(),
        user_id: auth.user_id,
        space_id: None,
        project_id: None,
        episode_id: body.episode_id,
        entity_ids: body.entity_ids.clone(),
        chunker_options: None,
    };
    let result = ingestion::ingest(&state.db, &raw_text, options, None).await?;
    Ok(Json(json!({
        "ingested": result.total_memories, "source": ingest_source, "title": title,
        "chunks_processed": result.total_chunks, "errors": result.errors,
    })))
}

/// SSE streaming variant of `/ingest`.
///
/// Emits one event per pipeline phase (detected, parsed, chunked, processed)
/// then either `done` or `error`. Falls back to a single `result` event when
/// the client does not send `Accept: text/event-stream`.
async fn ingest_text_stream(
    State(state): State<AppState>,
    Auth(auth): Auth,
    headers: HeaderMap,
    Json(body): Json<IngestBody>,
) -> Result<impl IntoResponse, AppError> {
    if body.url.is_none() && body.text.is_none() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "Provide url or text parameter".to_string(),
        )));
    }
    let raw_text = match body.text.as_ref() {
        Some(text) if !text.trim().is_empty() => {
            if text.len() > MAX_INGEST_TEXT_BYTES {
                return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
                    "text exceeds {} bytes; split the ingest",
                    MAX_INGEST_TEXT_BYTES
                ))));
            }
            text.trim().to_string()
        }
        Some(_) => {
            return Err(AppError(kleos_lib::EngError::InvalidInput(
                "text must be a non-empty string".to_string(),
            )));
        }
        None => {
            return Err(AppError(kleos_lib::EngError::Internal(
                "URL fetching not yet implemented in Rust port".to_string(),
            )));
        }
    };
    let ingest_source = body.source.clone().unwrap_or_else(|| "text".to_string());
    let title = body.title.clone().unwrap_or_else(|| {
        let t: String = raw_text.chars().take(60).collect();
        t.replace(char::from(10u8), " ")
    });

    let options = IngestOptions {
        mode: IngestMode::Raw,
        format: None,
        source: ingest_source,
        category: "general".to_string(),
        user_id: auth.user_id,
        space_id: None,
        project_id: None,
        episode_id: body.episode_id,
        entity_ids: body.entity_ids.clone(),
        chunker_options: None,
    };

    let accepts_sse = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));

    if !accepts_sse {
        let result = ingestion::ingest(&state.db, &raw_text, options, None).await?;
        let payload = json!({
            "ingested": result.total_memories, "title": title,
            "chunks_processed": result.total_chunks, "errors": result.errors,
            "job_id": result.job_id, "status": result.status,
        });
        return Ok(Sse::new(futures::stream::once(async move {
            Ok::<_, Infallible>(
                Event::default()
                    .event("result")
                    .json_data(payload)
                    .unwrap_or_else(|_| Event::default().data("{}")),
            )
        }))
        .keep_alive(KeepAlive::default())
        .into_response());
    }

    let (progress_tx, progress_rx) = tokio::sync::mpsc::unbounded_channel::<IngestProgressEvent>();
    let (sse_tx, sse_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    // Spawn ingestion task.
    let db = state.db.clone();
    let sse_tx_clone = sse_tx.clone();
    tokio::spawn(async move {
        let res = ingestion::ingest_streaming(&db, &raw_text, options, None, progress_tx).await;
        if let Err(e) = res {
            let _ = sse_tx_clone.send(
                Event::default()
                    .event("error")
                    .json_data(json!({"error": e.to_string()}))
                    .unwrap_or_else(|_| Event::default().data("{}")),
            );
        }
        // On success the pipeline already emitted IngestProgressEvent::Done,
        // which the relay forwards as a `progress` event with type=done.
    });

    // Spawn relay: progress -> SSE.
    tokio::spawn(async move {
        let mut rx = progress_rx;
        while let Some(evt) = rx.recv().await {
            let sse_event = Event::default()
                .event("progress")
                .json_data(&evt)
                .unwrap_or_else(|_| Event::default().data("{}"));
            if sse_tx.send(sse_event).is_err() {
                break;
            }
        }
    });

    let stream = futures::stream::unfold(sse_rx, |mut rx| async move {
        rx.recv().await.map(|evt| (Ok::<_, Infallible>(evt), rx))
    });

    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn add_conversation(
    _state: State<AppState>,
    Auth(_auth): Auth,
    Json(_body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Err(AppError(kleos_lib::EngError::NotImplemented(
        "/add requires LLM-based fact extraction (not yet implemented)".to_string(),
    )))
}

async fn derive(
    _state: State<AppState>,
    Auth(_auth): Auth,
    Json(_body): Json<Value>,
) -> Result<Json<Value>, AppError> {
    Err(AppError(kleos_lib::EngError::NotImplemented(
        "/derive requires LLM-based inference (not yet implemented)".to_string(),
    )))
}
