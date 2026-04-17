// ============================================================================
// Centralized validation constants and helpers
//
// All hard limits live here so they can be tuned from one place.
// Individual modules re-export or reference these instead of defining
// their own copies.
// ============================================================================

use crate::{EngError, Result};

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/// Maximum byte length of memory content (store + update).
pub const MAX_CONTENT_SIZE: usize = 102_400; // 100 KB

/// Hard cap on search/list result count.
pub const MAX_SEARCH_LIMIT: usize = 100;

/// Default result count when caller omits `limit`.
pub const DEFAULT_SEARCH_LIMIT: usize = 10;

/// Maximum character length of a full-text search query.
pub const MAX_FTS_QUERY_LEN: usize = 4096;

/// Top-K candidates passed to the reranker.
pub const RERANKER_TOP_K: usize = 12;

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Token budget ceiling for context assembly.
pub const MAX_TOKEN_BUDGET: usize = 64_000;

/// Maximum chars kept per entry when building context.
pub const MAX_CONTEXT_CHARS: usize = 4_000;

// ---------------------------------------------------------------------------
// Graph
// ---------------------------------------------------------------------------

/// Maximum relationships fetched per entity.
pub const MAX_ENTITY_RELATIONSHIPS: usize = 1_000;

/// Maximum nodes processed in community detection.
pub const MAX_COMMUNITY_NODES: usize = 10_000;

/// Maximum iterations in community detection.
pub const MAX_COMMUNITY_ITERATIONS: usize = 100;

/// Maximum iterations in per-tenant PageRank.
pub const MAX_PAGERANK_ITERATIONS: usize = 25;

// ---------------------------------------------------------------------------
// Skills / sessions / conversations
// ---------------------------------------------------------------------------

/// Hard cap on skills returned in a list.
pub const MAX_SKILLS_LIMIT: usize = 500;

/// Session list cap.
pub const MAX_SESSION_LIST: usize = 500;

/// Default session list page size.
pub const DEFAULT_SESSION_LIST: usize = 50;

/// Conversation list cap.
pub const MAX_CONVERSATION_LIST: usize = 100;

/// Default conversation list page size.
pub const DEFAULT_CONVERSATION_LIST: usize = 20;

// ---------------------------------------------------------------------------
// Ingestion / parsing
// ---------------------------------------------------------------------------

/// Maximum raw XML bytes from DOCX.
pub const MAX_DOCX_XML_BYTES: usize = 100 * 1024 * 1024;

/// Maximum single ZIP entry size.
pub const MAX_ZIP_ENTRY_SIZE: usize = 50 * 1024 * 1024;

/// Maximum total uncompressed ZIP size.
pub const MAX_ZIP_AGGREGATE_SIZE: usize = 500 * 1024 * 1024;

/// Maximum input bytes for PDF parsing.
pub const MAX_PDF_INPUT_BYTES: usize = 100 * 1024 * 1024;

/// Maximum extracted text bytes from PDF.
pub const MAX_PDF_TEXT_BYTES: usize = 100 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Intelligence
// ---------------------------------------------------------------------------

/// Minimum content length for fact decomposition.
pub const MIN_DECOMPOSITION_LENGTH: usize = 50;

/// Maximum facts extracted per decomposition call.
pub const MAX_DECOMPOSITION_FACTS: usize = 10;

// ---------------------------------------------------------------------------
// Shell / grounding
// ---------------------------------------------------------------------------

/// Maximum bytes of shell command output captured.
pub const MAX_SHELL_OUTPUT_BYTES: usize = 100_000;

/// Maximum lines of shell output.
pub const MAX_SHELL_OUTPUT_LINES: usize = 10_000;

// ---------------------------------------------------------------------------
// Gate / guard
// ---------------------------------------------------------------------------

/// Maximum character length of a gate pattern.
pub const MAX_PATTERN_CHARS: usize = 4_096;

// ---------------------------------------------------------------------------
// Artifact
// ---------------------------------------------------------------------------

/// Maximum bytes of artifact content indexed for FTS.
pub const ARTIFACT_FTS_MAX_SIZE: usize = 1_048_576;

// ---------------------------------------------------------------------------
// Tenant
// ---------------------------------------------------------------------------

/// Maximum length for a direct (non-hashed) tenant ID.
pub const MAX_TENANT_ID_LENGTH: usize = 64;

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

/// Maximum length of activity report agent field.
pub const MAX_ACTIVITY_AGENT_LEN: usize = 100;

/// Maximum length of activity report action field.
pub const MAX_ACTIVITY_ACTION_LEN: usize = 100;

/// Maximum length of activity report summary field.
pub const MAX_ACTIVITY_SUMMARY_LEN: usize = 10_000;

// ---------------------------------------------------------------------------
// Batch endpoint
// ---------------------------------------------------------------------------

/// Maximum ops in a single batch request.
pub const MAX_BATCH_OPS: usize = 100;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Validate content is non-empty and within size limit.
pub fn validate_content(content: &str) -> Result<()> {
    if content.trim().is_empty() {
        return Err(EngError::InvalidInput(
            "content must not be empty".to_string(),
        ));
    }
    if content.len() > MAX_CONTENT_SIZE {
        return Err(EngError::InvalidInput(format!(
            "content exceeds maximum size of {} bytes",
            MAX_CONTENT_SIZE
        )));
    }
    Ok(())
}

/// Clamp a user-supplied limit to [1, MAX_SEARCH_LIMIT], defaulting to DEFAULT_SEARCH_LIMIT.
pub fn clamp_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT)
}

/// Validate a string field length.
pub fn validate_string_len(field: &str, value: &str, max: usize) -> Result<()> {
    if value.len() > max {
        return Err(EngError::InvalidInput(format!(
            "{} exceeds maximum length of {} chars",
            field, max
        )));
    }
    Ok(())
}
