// ============================================================================
// Ingestion pipeline orchestrator -- ported from ingestion/index.ts
// ============================================================================

pub mod chunker;
pub mod detect;
pub mod parsers;
pub mod processors;
pub mod types;

use crate::db::Database;
use crate::Result;
use sha2::{Digest, Sha256};
use std::time::Instant;
use types::{
    Chunk, FormatMeta, IngestOptions, IngestProgressEvent, IngestProgressSender, IngestResult,
    IngestStatus, ProcessOptions,
};
use uuid::Uuid;

pub use chunker::chunk_document;
pub use detect::detect_format;

/// Compute SHA-256 hex digest of input bytes for dedup.
fn content_hash(input: &[u8]) -> String {
    let hash = Sha256::digest(input);
    hex::encode(hash)
}

/// Check if this content has already been ingested for the given user.
/// Returns true if a matching hash exists (skip this ingestion).
async fn is_duplicate(db: &Database, hash: &str, user_id: i64) -> bool {
    let h = hash.to_string();
    db.read(move |conn| {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM ingestion_hashes WHERE sha256 = ?1 AND user_id = ?2",
                rusqlite::params![h, user_id],
                |_| Ok(true),
            )
            .unwrap_or(false);
        Ok(exists)
    })
    .await
    .unwrap_or(false)
}

/// Record a content hash after successful ingestion.
async fn record_hash(db: &Database, hash: &str, user_id: i64, job_id: &str) {
    let h = hash.to_string();
    let j = job_id.to_string();
    let _ = db
        .write(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO ingestion_hashes (sha256, user_id, job_id) VALUES (?1, ?2, ?3)",
                rusqlite::params![h, user_id, j],
            )
            .map_err(|e| crate::EngError::DatabaseMessage(e.to_string()))
        })
        .await;
}

/// Emit a progress event if `tx` is Some; silently ignore closed channels.
fn emit(tx: &Option<IngestProgressSender>, event: IngestProgressEvent) {
    if let Some(ref tx) = tx {
        let _ = tx.send(event);
    }
}

/// Run the ingestion pipeline: detect format -> parse -> chunk -> process -> store.
///
/// This is the main entry point for bulk document ingestion.
/// Ported from the TS runPipeline() function.
#[tracing::instrument(skip(db, input, options, meta), fields(input_len = input.len()))]
pub async fn ingest(
    db: &Database,
    input: &str,
    options: IngestOptions,
    meta: Option<&FormatMeta>,
) -> Result<IngestResult> {
    ingest_inner(db, input, options, meta, None).await
}

/// Streaming variant: same as [`ingest`] but emits [`IngestProgressEvent`]s as
/// each pipeline phase completes.
#[tracing::instrument(skip(db, input, options, meta, progress_tx), fields(input_len = input.len()))]
pub async fn ingest_streaming(
    db: &Database,
    input: &str,
    options: IngestOptions,
    meta: Option<&FormatMeta>,
    progress_tx: IngestProgressSender,
) -> Result<IngestResult> {
    ingest_inner(db, input, options, meta, Some(progress_tx)).await
}

async fn ingest_inner(
    db: &Database,
    input: &str,
    options: IngestOptions,
    meta: Option<&FormatMeta>,
    progress_tx: Option<IngestProgressSender>,
) -> Result<IngestResult> {
    let start = Instant::now();
    let job_id = format!("ingest_{}", &Uuid::new_v4().to_string()[..8]);
    let mut errors: Vec<String> = Vec::new();
    let mut total_chunks: usize = 0;
    let mut total_memories: usize = 0;

    // Dedup: skip if this exact content was already ingested for this user.
    let hash = content_hash(input.as_bytes());
    if is_duplicate(db, &hash, options.user_id).await {
        emit(
            &progress_tx,
            IngestProgressEvent::Skipped {
                job_id: job_id.clone(),
                reason: "duplicate content".into(),
            },
        );
        return Ok(IngestResult {
            job_id,
            status: IngestStatus::Skipped,
            total_documents: 0,
            total_chunks: 0,
            total_memories: 0,
            errors: vec!["duplicate content -- already ingested".to_string()],
            duration_ms: start.elapsed().as_millis(),
        });
    }

    // 1. Detect format
    let format = options
        .format
        .unwrap_or_else(|| detect_format(input.as_bytes(), meta));
    emit(
        &progress_tx,
        IngestProgressEvent::Detected {
            job_id: job_id.clone(),
            format: format.to_string(),
        },
    );

    // 2. Parse documents
    let docs = match parsers::parse_with_format(format, input) {
        Ok(d) => d,
        Err(e) => {
            let msg = format!("Parser error: {}", e);
            emit(
                &progress_tx,
                IngestProgressEvent::Error {
                    job_id: job_id.clone(),
                    message: msg.clone(),
                },
            );
            return Ok(IngestResult {
                job_id,
                status: IngestStatus::Failed,
                total_documents: 0,
                total_chunks: 0,
                total_memories: 0,
                errors: vec![msg],
                duration_ms: start.elapsed().as_millis(),
            });
        }
    };

    let total_documents = docs.len();
    emit(
        &progress_tx,
        IngestProgressEvent::Parsed {
            job_id: job_id.clone(),
            total_documents,
        },
    );

    // 3. Process each document: chunk -> process
    let chunker_opts = options.chunker_options.as_ref();

    for (doc_idx, doc) in docs.iter().enumerate() {
        // Chunk the document
        let doc_chunks: Vec<Chunk> =
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                chunk_document(doc, chunker_opts)
            })) {
                Ok(chunks) => chunks,
                Err(_) => {
                    errors.push(format!("Document \"{}\": chunking error", doc.title));
                    continue;
                }
            };

        let doc_chunk_count = doc_chunks.len();
        total_chunks += doc_chunk_count;
        emit(
            &progress_tx,
            IngestProgressEvent::Chunked {
                job_id: job_id.clone(),
                document_index: doc_idx,
                document_title: doc.title.clone(),
                chunks: doc_chunk_count,
            },
        );

        // Build process options
        let process_options = ProcessOptions {
            source: options.source.clone(),
            category: options.category.clone(),
            user_id: options.user_id,
            space_id: options.space_id,
            project_id: options.project_id,
            episode_id: options.episode_id,
            entity_ids: options.entity_ids.clone(),
        };

        let mut doc_memories: usize = 0;
        let mut chunks_done: usize = 0;

        // Process chunks (one at a time matching TS behavior)
        for chunk in &doc_chunks {
            let result = processors::process_chunks(
                db,
                options.mode,
                std::slice::from_ref(chunk),
                &process_options,
            )
            .await;

            doc_memories += result.memories_created;
            total_memories += result.memories_created;
            errors.extend(result.errors);
            chunks_done += 1;
        }

        emit(
            &progress_tx,
            IngestProgressEvent::Processed {
                job_id: job_id.clone(),
                document_index: doc_idx,
                memories_created: doc_memories,
                chunks_done,
                chunks_total: doc_chunk_count,
            },
        );
    }

    let duration_ms = start.elapsed().as_millis();

    tracing::info!(
        "ingestion complete: {} docs, {} chunks, {} memories in {}ms",
        total_documents,
        total_chunks,
        total_memories,
        duration_ms
    );

    // Record hash so future identical content is skipped.
    record_hash(db, &hash, options.user_id, &job_id).await;

    emit(
        &progress_tx,
        IngestProgressEvent::Done {
            job_id: job_id.clone(),
            total_documents,
            total_chunks,
            total_memories,
            duration_ms,
        },
    );

    Ok(IngestResult {
        job_id,
        status: IngestStatus::Completed,
        total_documents,
        total_chunks,
        total_memories,
        errors,
        duration_ms,
    })
}

/// Ingest with binary input (for PDF, DOCX, ZIP formats).
#[tracing::instrument(skip(db, input, options, meta), fields(input_bytes = input.len()))]
pub async fn ingest_binary(
    db: &Database,
    input: &[u8],
    options: IngestOptions,
    meta: Option<&FormatMeta>,
) -> Result<IngestResult> {
    let start = Instant::now();
    let job_id = format!("ingest_{}", &Uuid::new_v4().to_string()[..8]);

    // Detect format
    let format = options.format.unwrap_or_else(|| detect_format(input, meta));

    // For text formats, convert and delegate
    if parsers::is_text_format(format) {
        let text = std::str::from_utf8(input).map_err(|e| {
            crate::EngError::InvalidInput(format!("input is not valid UTF-8: {}", e))
        })?;
        return ingest(db, text, options, meta).await;
    }

    // Dedup: skip if this exact binary content was already ingested.
    let hash = content_hash(input);
    if is_duplicate(db, &hash, options.user_id).await {
        return Ok(IngestResult {
            job_id,
            status: IngestStatus::Skipped,
            total_documents: 0,
            total_chunks: 0,
            total_memories: 0,
            errors: vec!["duplicate content -- already ingested".to_string()],
            duration_ms: start.elapsed().as_millis(),
        });
    }

    // Binary format parsing
    let docs = match parsers::parse_binary_with_format(format, input) {
        Ok(d) => d,
        Err(e) => {
            let msg = format!("Parser error: {}", e);
            return Ok(IngestResult {
                job_id,
                status: IngestStatus::Failed,
                total_documents: 0,
                total_chunks: 0,
                total_memories: 0,
                errors: vec![msg],
                duration_ms: start.elapsed().as_millis(),
            });
        }
    };

    // Same chunk + process pipeline as text ingestion
    let mut errors = Vec::new();
    let mut total_chunks = 0;
    let mut total_memories = 0;

    for doc in &docs {
        let doc_chunks = chunk_document(doc, options.chunker_options.as_ref());
        total_chunks += doc_chunks.len();

        let process_options = ProcessOptions {
            source: options.source.clone(),
            category: options.category.clone(),
            user_id: options.user_id,
            space_id: options.space_id,
            project_id: options.project_id,
            episode_id: options.episode_id,
            entity_ids: options.entity_ids.clone(),
        };

        for chunk in &doc_chunks {
            let result = processors::process_chunks(
                db,
                options.mode,
                std::slice::from_ref(chunk),
                &process_options,
            )
            .await;
            total_memories += result.memories_created;
            errors.extend(result.errors);
        }
    }

    // Record hash so future identical content is skipped.
    record_hash(db, &hash, options.user_id, &job_id).await;

    Ok(IngestResult {
        job_id,
        status: IngestStatus::Completed,
        total_documents: docs.len(),
        total_chunks,
        total_memories,
        errors,
        duration_ms: start.elapsed().as_millis(),
    })
}
