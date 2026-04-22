// ============================================================================
// Raw processor -- ported from processors/raw.ts
// ============================================================================
//
// Stores each chunk directly as a memory using the canonical memory::store
// path, so every ingested chunk goes through the same pipeline as POST /store:
// SimHash dedup, FTS5 index, LanceDB vector insert (when an embedder is
// provided), valence analysis, pagerank dirty-mark, and a durable
// `ingestion.fact_extract` job that runs `fast_extract_facts` through the
// jobs queue (retryable, survives restart).

use crate::db::Database;
use crate::ingestion::types::{Chunk, IngestContext, ProcessOptions, ProcessResult};
use crate::jobs::enqueue_job;
use crate::memory::{self, types::StoreRequest};
use std::sync::Arc;

/// Process chunks by storing each as a memory via `memory::store`.
///
/// The embedder in `ctx` is used to compute a per-chunk vector before the
/// insert so `memory::store` can forward it to the LanceDB index. When no
/// embedder is configured the memory is still persisted but vector search
/// for it will only match after a later backfill.
#[tracing::instrument(skip(db, ctx, chunks, options), fields(chunk_count = chunks.len()))]
pub async fn process(
    db: Arc<Database>,
    ctx: &IngestContext,
    chunks: &[Chunk],
    options: &ProcessOptions,
) -> ProcessResult {
    let mut memories_created = 0;
    let mut errors = Vec::new();

    for chunk in chunks {
        let content = chunk.text.trim();
        if content.is_empty() {
            errors.push(format!("Chunk {}: empty after trim", chunk.index));
            continue;
        }

        let embedding = match &ctx.embedder {
            Some(embedder) => match embedder.embed(content).await {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        "ingestion embedder failed for chunk {}: {} -- continuing without vector",
                        chunk.index,
                        e
                    );
                    None
                }
            },
            None => None,
        };

        let req = StoreRequest {
            content: content.to_string(),
            category: options.category.clone(),
            source: options.source.clone(),
            importance: 5,
            tags: None,
            embedding,
            session_id: None,
            is_static: None,
            user_id: Some(options.user_id),
            space_id: options.space_id,
            parent_memory_id: None,
        };

        match memory::store(db.as_ref(), req).await {
            Ok(result) => {
                if result.duplicate_of.is_some() {
                    continue;
                }
                memories_created += 1;
                let payload = serde_json::json!({
                    "memory_id": result.id,
                    "content": content,
                    "user_id": options.user_id,
                    "episode_id": options.episode_id,
                });
                if let Err(e) = enqueue_job(
                    db.as_ref(),
                    "ingestion.fact_extract",
                    &payload.to_string(),
                    3,
                )
                .await
                {
                    tracing::warn!(
                        memory_id = result.id,
                        "failed to enqueue ingestion.fact_extract job: {}",
                        e
                    );
                }
            }
            Err(e) => {
                errors.push(format!("Chunk {}: insert failed: {}", chunk.index, e));
            }
        }
    }

    ProcessResult {
        memories_created,
        errors,
    }
}
