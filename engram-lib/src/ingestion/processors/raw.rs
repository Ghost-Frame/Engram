// ============================================================================
// Raw processor -- ported from processors/raw.ts
// ============================================================================
//
// Stores each chunk directly as a memory. The TS version also does:
// - Embedding computation
// - SimHash deduplication
// - Post-store job enqueueing
// These integrations will be wired up when the embedding and job systems
// are ported. For now, we store the memory directly via DB insert.

use crate::db::Database;
use crate::ingestion::types::{Chunk, ProcessOptions, ProcessResult};
use uuid::Uuid;

/// Process chunks by storing each as a raw memory.
pub async fn process(
    db: &Database,
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

        let sync_id = Uuid::new_v4().to_string();

        match db
            .conn
            .execute(
                "INSERT INTO memories (content, category, source, importance, user_id, space_id, \
                 episode_id, sync_id, confidence, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, 5, ?4, ?5, ?6, ?7, 1.0, datetime('now'), datetime('now'))",
                libsql::params![
                    content.to_string(),
                    options.category.clone(),
                    options.source.clone(),
                    options.user_id,
                    options.space_id,
                    options.episode_id,
                    sync_id
                ],
            )
            .await
        {
            Ok(_) => {
                memories_created += 1;
                // TODO: Compute embedding and write to vec table
                // TODO: SimHash deduplication check
                // TODO: Enqueue post_store job for FSRS init, entity linking, etc.
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
