// ============================================================================
// Extract processor -- ported from processors/extract.ts
// ============================================================================
//
// The TS version sends each chunk through LLM fact extraction before storing.
// Since the LLM integration is not yet ported, this processor falls back to
// raw storage behavior while preserving the extraction pipeline structure.
//
// When LLM is available, this will:
// 1. Build extraction prompt per chunk
// 2. Call LLM to extract facts
// 3. Parse JSON response
// 4. Store each extracted fact as a separate memory with embeddings

use crate::db::Database;
use crate::ingestion::types::{Chunk, ProcessOptions, ProcessResult};

/// Process chunks using LLM fact extraction.
/// Currently falls back to raw processing since LLM is not available.
pub async fn process(db: &Database, chunks: &[Chunk], options: &ProcessOptions) -> ProcessResult {
    // TODO: When LLM integration is ported, implement:
    // 1. Build extraction prompt with chunk context
    // 2. Call callLocalModel() with the prompt
    // 3. Parse JSON response to extract facts
    // 4. Deduplicate via SimHash
    // 5. Store each fact as a memory with embeddings
    //
    // For now, fall back to raw processing
    tracing::warn!(
        "extract processor: LLM not available, falling back to raw storage for {} chunks",
        chunks.len()
    );

    super::raw::process(db, chunks, options).await
}
