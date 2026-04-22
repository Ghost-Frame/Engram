// ============================================================================
// Processor registry -- ported from processors/
// ============================================================================

pub mod extract;
pub mod raw;

use crate::db::Database;
use crate::ingestion::types::{Chunk, IngestContext, IngestMode, ProcessOptions, ProcessResult};
use std::sync::Arc;

/// Process chunks using the specified mode.
#[tracing::instrument(skip(db, ctx, chunks, options), fields(chunk_count = chunks.len()))]
pub async fn process_chunks(
    db: Arc<Database>,
    ctx: &IngestContext,
    mode: IngestMode,
    chunks: &[Chunk],
    options: &ProcessOptions,
) -> ProcessResult {
    match mode {
        IngestMode::Extract => extract::process(db, ctx, chunks, options).await,
        IngestMode::Raw => raw::process(db, ctx, chunks, options).await,
    }
}
