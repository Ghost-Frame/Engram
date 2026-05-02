// ============================================================================
// Extract processor -- ported from processors/extract.ts
// ============================================================================
//
// Asks the configured local LLM to pull structured facts out of each chunk
// and stores each fact as its own memory through `memory::store`, inheriting
// the SimHash / FTS / vector / valence pipeline from the raw processor.
//
// When no LLM client is configured (or it is unreachable / circuit-open) the
// processor degrades to raw ingestion so the caller still gets a persisted
// chunk. A warning is logged so operators can see the degradation.

use crate::db::Database;
use crate::ingestion::types::{Chunk, IngestContext, ProcessOptions, ProcessResult};
use crate::jobs::enqueue_job;
use crate::llm::{local::LocalModelClient, repair_and_parse_json};
use crate::memory::{self, types::StoreRequest};
use std::sync::Arc;

const EXTRACT_SYSTEM_PROMPT: &str = "You are a precise fact extraction engine. Read the provided text and return a JSON array of concise, atomic statements that capture the durable facts, decisions, preferences, and entities mentioned. Each array entry must be a single plain-English sentence. Do not include commentary, markdown, or any field other than the JSON array. Return `[]` if there is nothing to extract.";

const MIN_FACT_LEN: usize = 5;
const MAX_FACT_LEN: usize = 512;

/// Process chunks using LLM fact extraction.
///
/// If `ctx.llm` is absent or marked unavailable (probe failed / circuit open)
/// the processor falls back to `raw::process` so ingestion still persists the
/// chunk text verbatim. On fallback a warning is logged once per call.
#[tracing::instrument(skip(db, ctx, chunks, options), fields(chunk_count = chunks.len()))]
pub async fn process(
    db: Arc<Database>,
    ctx: &IngestContext,
    chunks: &[Chunk],
    options: &ProcessOptions,
) -> ProcessResult {
    let llm = match ctx.llm.as_ref() {
        Some(client) if client.is_available() => Arc::clone(client),
        _ => {
            tracing::warn!(
                "extract processor: LLM unavailable, falling back to raw storage for {} chunks",
                chunks.len()
            );
            return super::raw::process(db, ctx, chunks, options).await;
        }
    };

    let mut memories_created = 0;
    let mut errors = Vec::new();

    for chunk in chunks {
        let content = chunk.text.trim();
        if content.is_empty() {
            errors.push(format!("Chunk {}: empty after trim", chunk.index));
            continue;
        }

        let facts = match extract_facts(&llm, content).await {
            Ok(facts) => facts,
            Err(e) => {
                tracing::warn!(
                    "extract processor: LLM call failed for chunk {}: {} -- falling back to raw for this chunk",
                    chunk.index,
                    e
                );
                let single = std::slice::from_ref(chunk);
                let fallback = super::raw::process(Arc::clone(&db), ctx, single, options).await;
                memories_created += fallback.memories_created;
                errors.extend(fallback.errors);
                continue;
            }
        };

        if facts.is_empty() {
            tracing::debug!(
                chunk = chunk.index,
                "extract processor: LLM returned zero facts, storing raw chunk as fallback"
            );
            let single = std::slice::from_ref(chunk);
            let fallback = super::raw::process(Arc::clone(&db), ctx, single, options).await;
            memories_created += fallback.memories_created;
            errors.extend(fallback.errors);
            continue;
        }

        for fact in facts {
            let req = StoreRequest {
                content: fact.clone(),
                category: options.category.clone(),
                source: options.source.clone(),
                importance: 5,
                tags: None,
                embedding: None,
                session_id: None,
                is_static: None,
                user_id: Some(options.user_id),
                space_id: options.space_id,
                parent_memory_id: None,
                chunk_embeddings: None,
            };

            let store_outcome = match &ctx.embedder {
                Some(embedder) => {
                    memory::store_with_chunks(db.as_ref(), embedder.as_ref(), req).await
                }
                None => memory::store(db.as_ref(), req).await,
            };

            match store_outcome {
                Ok(result) => {
                    if result.duplicate_of.is_some() {
                        continue;
                    }
                    memories_created += 1;
                    let payload = serde_json::json!({
                        "memory_id": result.id,
                        "content": fact,
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
                            "failed to enqueue ingestion.fact_extract job (extract mode): {}",
                            e
                        );
                    }
                }
                Err(e) => {
                    errors.push(format!(
                        "Chunk {}: extracted-fact insert failed: {}",
                        chunk.index, e
                    ));
                }
            }
        }
    }

    ProcessResult {
        memories_created,
        errors,
    }
}

async fn extract_facts(llm: &LocalModelClient, chunk_text: &str) -> crate::Result<Vec<String>> {
    let response = llm
        .call(EXTRACT_SYSTEM_PROMPT, chunk_text, None)
        .await
        .map_err(|e| crate::EngError::Internal(format!("extract LLM call failed: {}", e)))?;

    let parsed = repair_and_parse_json(&response).ok_or_else(|| {
        crate::EngError::Internal(format!(
            "extract LLM returned no parseable JSON (len={})",
            response.len()
        ))
    })?;

    let array = parsed.as_array().ok_or_else(|| {
        crate::EngError::Internal("extract LLM response was not a JSON array".into())
    })?;

    let mut facts = Vec::with_capacity(array.len());
    for value in array {
        let candidate = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let trimmed = candidate.trim();
        if trimmed.len() < MIN_FACT_LEN || trimmed.len() > MAX_FACT_LEN {
            continue;
        }
        facts.push(trimmed.to_string());
    }

    Ok(facts)
}
