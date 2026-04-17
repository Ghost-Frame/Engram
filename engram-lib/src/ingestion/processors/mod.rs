// ============================================================================
// Processor registry -- ported from processors/
// ============================================================================

pub mod extract;
pub mod raw;

use crate::db::Database;
use crate::ingestion::types::{Chunk, IngestMode, ProcessOptions, ProcessResult};

/// Process chunks using the specified mode.
#[tracing::instrument(skip(db, chunks, options), fields(chunk_count = chunks.len()))]
pub async fn process_chunks(
    db: &Database,
    mode: IngestMode,
    chunks: &[Chunk],
    options: &ProcessOptions,
) -> ProcessResult {
    match mode {
        IngestMode::Extract => extract::process(db, chunks, options).await,
        IngestMode::Raw => raw::process(db, chunks, options).await,
    }
}
