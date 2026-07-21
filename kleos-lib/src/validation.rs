// ============================================================================
// Centralized validation constants and helpers
//
// All hard limits live here so they can be tuned from one place.
// Individual modules re-export or reference these instead of defining
// their own copies.
// ============================================================================

use crate::{EngError, Result};

// --- Memory ---

/// Maximum byte length of memory content (store + update).
pub const MAX_CONTENT_SIZE: usize = 102_400; // 100 KB

/// Hard cap on search/list result count.
pub const MAX_SEARCH_LIMIT: usize = 100;

/// Default result count when caller omits `limit`.
pub const DEFAULT_SEARCH_LIMIT: usize = 10;

/// Maximum character length of a full-text search query.
pub const MAX_FTS_QUERY_LEN: usize = 4096;

/// Top-K candidates passed to the reranker.
pub const RERANKER_TOP_K: usize = 24;

// --- Context ---

/// Token budget ceiling for context assembly.
pub const MAX_TOKEN_BUDGET: usize = 64_000;

/// Maximum static memories injected verbatim into a context assembly.
/// Bounds per-assembly cost when a user accumulates a large static corpus;
/// selection is top-N by importance and the token budget trims further.
pub const MAX_STATIC_CONTEXT_MEMORIES: usize = 200;

/// Maximum chars kept per entry when building context.
pub const MAX_CONTEXT_CHARS: usize = 4_000;

// --- Graph ---

/// Maximum relationships fetched per entity.
pub const MAX_ENTITY_RELATIONSHIPS: usize = 1_000;

/// Maximum nodes processed in community detection.
pub const MAX_COMMUNITY_NODES: usize = 10_000;

/// Maximum iterations in community detection.
pub const MAX_COMMUNITY_ITERATIONS: usize = 100;

/// Maximum iterations in per-tenant PageRank.
pub const MAX_PAGERANK_ITERATIONS: usize = 25;

/// Maximum nodes materialized in a single `build_graph` request.
/// DoS bound: clamps caller-supplied node cap so one request cannot force the
/// server to materialize an arbitrarily large graph.
pub const MAX_GRAPH_BUILD_NODES: usize = 5_000;

/// Maximum depth accepted for k-hop neighborhood traversal.
/// DoS bound: neighborhood expansion is super-linear in depth; this cap
/// prevents a single request from amplifying into a full graph traversal.
pub const MAX_GRAPH_NEIGHBORHOOD_DEPTH: u32 = 5;

/// Maximum entities returned for a single memory.
/// DoS bound: caps entity fan-out per memory (i64 for direct rusqlite binding).
pub const MAX_MEMORY_ENTITY_FANOUT: i64 = 1_000;

/// Maximum entities per memory considered for pairwise co-occurrence recording.
/// DoS bound: co-occurrence pairing is O(n^2) in entity count; a single 100KB
/// memory stuffed with quoted tokens or acronyms could otherwise force hundreds
/// of thousands of row upserts from one store call.
pub const MAX_COOCCURRENCE_ENTITIES: usize = 50;

// --- Pagination ---

/// Maximum accepted pagination offset across list endpoints.
/// Absurd offsets are rejected up-front to avoid needless SQL scans.
pub const MAX_PAGINATION_OFFSET: usize = 1_000_000;

// --- Skills / sessions / conversations ---

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

// --- Ingestion / parsing ---

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

/// Maximum raw input bytes accepted by `ingest_binary`.
pub const MAX_INGEST_INPUT_BYTES: usize = 100 * 1024 * 1024; // 100 MiB

// --- Intelligence ---

/// Minimum content length for fact decomposition.
pub const MIN_DECOMPOSITION_LENGTH: usize = 50;

/// Maximum facts extracted per decomposition call.
pub const MAX_DECOMPOSITION_FACTS: usize = 10;

// --- Shell / grounding ---

/// Maximum bytes of shell command output captured.
pub const MAX_SHELL_OUTPUT_BYTES: usize = 100_000;

/// Maximum lines of shell output.
pub const MAX_SHELL_OUTPUT_LINES: usize = 10_000;

// --- Gate / guard ---

/// Maximum character length of a gate pattern.
pub const MAX_PATTERN_CHARS: usize = 4_096;

// --- Artifact ---

/// Maximum bytes of artifact content indexed for FTS.
pub const ARTIFACT_FTS_MAX_SIZE: usize = 1_048_576;

/// Artifacts larger than this threshold are stored on disk rather than inline
/// in SQLite. 1 MiB aligns with the D2 decision in the artifacts design doc.
pub const ARTIFACT_DISK_TIER_THRESHOLD: usize = 1_048_576;

// --- Tenant ---

/// Maximum length for a direct (non-hashed) tenant ID.
pub const MAX_TENANT_ID_LENGTH: usize = 64;

// --- Activity ---

/// Maximum length of activity report agent field.
pub const MAX_ACTIVITY_AGENT_LEN: usize = 100;

/// Maximum length of activity report action field.
pub const MAX_ACTIVITY_ACTION_LEN: usize = 100;

/// Maximum length of activity report summary field.
pub const MAX_ACTIVITY_SUMMARY_LEN: usize = 10_000;

// --- Spaces ---

/// Maximum spaces one user may own. Growth bound for the spaces table in
/// shared-monolith mode ([41]); generous relative to real usage.
pub const MAX_SPACES_PER_USER: i64 = 100;

// --- Batch endpoint ---

/// Maximum ops in a single batch request.
pub const MAX_BATCH_OPS: usize = 100;

// --- HTTP transport limits (server-side) ---
//
// These cap the size and shape of JSON request bodies before the handler sees
// them, and bound ingest/upload payloads. Centralizing here lets SDK
// generators surface the limits as part of the API contract.

/// Maximum recursion depth accepted by the JSON body middleware. Requests
/// with deeper nesting are rejected with 400 to prevent pathological
/// parser CPU usage.
pub const MAX_JSON_DEPTH: u32 = 64;

/// Maximum accumulated body bytes held in memory by the JSON depth
/// middleware. Requests beyond this return 413.
pub const MAX_JSON_BUFFER_BYTES: usize = 2 * 1024 * 1024; // 2 MiB

/// Maximum single artifact upload body size (`POST /artifacts`).
pub const MAX_ARTIFACT_UPLOAD_BYTES: usize = 50 * 1024 * 1024; // 50 MiB

/// Maximum rows accepted in a single import batch.
pub const MAX_IMPORT_BATCH: usize = 5_000;

/// Maximum bytes of raw text accepted by `/ingest` in a single request.
pub const MAX_INGEST_TEXT_BYTES: usize = 1 << 20; // 1 MiB

/// Maximum total bytes for a resumable/chunked upload session.
pub const MAX_UPLOAD_TOTAL_BYTES: i64 = 256 * 1024 * 1024; // 256 MiB

/// Maximum bytes per individual upload chunk.
pub const MAX_UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

// --- Helpers ---

/// Truncate a string to at most `max_bytes` bytes on a valid UTF-8 char boundary.
///
/// Returns a sub-slice that never splits a multibyte character. If `max_bytes`
/// falls inside a multibyte sequence, the slice ends before that character.
pub fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut i = max_bytes;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

/// Find the byte offset of the first ASCII-case-insensitive match of `needle`
/// in `haystack`, or `None` if `needle` is empty or absent.
///
/// Intended for ASCII needles (tag names, keywords). Because a full ASCII
/// match can only begin on an ASCII lead byte and spans only ASCII bytes, the
/// returned offset and `offset + needle.len()` are always valid UTF-8 char
/// boundaries in `haystack`. This lets callers locate ASCII markers without
/// building a parallel lowercased copy whose byte offsets can drift from the
/// original (some characters change byte length under `to_lowercase`).
pub fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let pat = needle.as_bytes();
    if pat.is_empty() || hay.len() < pat.len() {
        return None;
    }
    (0..=hay.len() - pat.len()).find(|&i| hay[i..i + pat.len()].eq_ignore_ascii_case(pat))
}

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

/// Lower bound of the supported memory importance range.
pub const MIN_IMPORTANCE: i64 = 1;

/// Upper bound of the supported memory importance range.
pub const MAX_IMPORTANCE: i64 = 10;

/// Clamp user-provided importance into the supported memory range.
///
/// Shared by `memory::store`/`memory::update` (via the i32 wrapper there) and
/// `inbox::edit_and_approve` so every write path agrees on the range.
pub fn clamp_importance_i64(value: i64) -> i64 {
    value.clamp(MIN_IMPORTANCE, MAX_IMPORTANCE)
}

/// Parse and normalize a caller-supplied JSON tag array into the canonical
/// stored form: trimmed, lowercased, empties dropped, serialized as a JSON
/// array. Returns `Ok(None)` when no usable tags remain after normalization
/// (callers should store NULL), and `Err` when the input is not a JSON array
/// of strings -- the memories.tags column must never hold non-JSON text.
pub fn normalize_tags_json(raw: &str) -> Result<Option<String>> {
    let parsed: Vec<String> = serde_json::from_str(raw)
        .map_err(|_| EngError::InvalidInput("tags must be a JSON array of strings".to_string()))?;
    let normalized: Vec<String> = parsed
        .iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if normalized.is_empty() {
        Ok(None)
    } else {
        // Vec<String> serialization cannot fail.
        Ok(Some(serde_json::to_string(&normalized).unwrap()))
    }
}

/// Clamp a user-supplied limit to [1, MAX_SEARCH_LIMIT], defaulting to DEFAULT_SEARCH_LIMIT.
pub fn clamp_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT)
}

/// Clamp a signed (i64) limit to [1, max], converting to usize.
///
/// Non-positive values map to 1; values above `max` clamp to `max`.
/// Use at route boundaries where the query/body field is `i64` to prevent
/// negative limits producing unbounded SQL queries or wrapping casts.
pub fn clamp_signed_limit(raw: i64, default: usize, max: usize) -> usize {
    if raw <= 0 {
        default.clamp(1, max)
    } else {
        (raw as usize).clamp(1, max)
    }
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

/// Validate a batch operation count is non-empty and within MAX_BATCH_OPS.
pub fn validate_batch_size(n: usize) -> Result<()> {
    if n == 0 {
        return Err(EngError::InvalidInput("ops must not be empty".to_string()));
    }
    if n > MAX_BATCH_OPS {
        return Err(EngError::InvalidInput(format!(
            "batch limited to {} ops, got {}",
            MAX_BATCH_OPS, n
        )));
    }
    Ok(())
}

/// Tests for content/limit/tag validation and clamping helpers.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies empty/whitespace-only content is rejected and oversized
    /// content is rejected, while valid content passes.
    #[test]
    fn validate_content_rejects_empty_and_too_large() {
        assert!(validate_content("").is_err());
        assert!(validate_content("   ").is_err());
        assert!(validate_content("ok").is_ok());
        let big = "a".repeat(MAX_CONTENT_SIZE + 1);
        assert!(validate_content(&big).is_err());
    }

    /// Verifies clamp_limit defaults when unset, floors at 1, and caps at
    /// MAX_SEARCH_LIMIT.
    #[test]
    fn clamp_limit_stays_in_range() {
        assert_eq!(clamp_limit(None), DEFAULT_SEARCH_LIMIT);
        assert_eq!(clamp_limit(Some(0)), 1);
        assert_eq!(clamp_limit(Some(5)), 5);
        assert_eq!(clamp_limit(Some(999)), MAX_SEARCH_LIMIT);
    }

    /// Verifies clamp_signed_limit maps non-positive values to the default
    /// and caps values above `max`.
    #[test]
    fn clamp_signed_limit_rejects_negative_and_caps() {
        assert_eq!(clamp_signed_limit(-1, 20, 100), 20);
        assert_eq!(clamp_signed_limit(0, 20, 100), 20);
        assert_eq!(clamp_signed_limit(i64::MIN, 20, 100), 20);
        assert_eq!(clamp_signed_limit(50, 20, 100), 50);
        assert_eq!(clamp_signed_limit(9_999_999, 20, 100), 100);
        assert_eq!(clamp_signed_limit(1, 20, 100), 1);
    }

    /// Verifies truncate_on_char_boundary never splits multibyte characters
    /// (snowman, emoji) across ASCII and non-ASCII inputs.
    #[test]
    fn truncate_on_char_boundary_never_panics() {
        // ASCII: straightforward
        assert_eq!(truncate_on_char_boundary("hello world", 5), "hello");
        assert_eq!(truncate_on_char_boundary("hello", 100), "hello");
        assert_eq!(truncate_on_char_boundary("", 10), "");
        // Multibyte: snowman is 3 bytes (E2 98 83), cut inside -> back up
        let s = "aa\u{2603}bb"; // 2 + 3 + 2 = 7 bytes
        assert_eq!(truncate_on_char_boundary(s, 5), "aa\u{2603}");
        assert_eq!(truncate_on_char_boundary(s, 3), "aa"); // lands inside snowman
        assert_eq!(truncate_on_char_boundary(s, 4), "aa"); // still inside snowman
        assert_eq!(truncate_on_char_boundary(s, 2), "aa");
        // 4-byte emoji
        let e = "x\u{1F600}y"; // 1 + 4 + 1 = 6 bytes
        assert_eq!(truncate_on_char_boundary(e, 2), "x"); // inside emoji
        assert_eq!(truncate_on_char_boundary(e, 5), "x\u{1F600}");
    }

    /// Verifies case-insensitive ASCII matching and that returned offsets
    /// remain valid UTF-8 char boundaries even with a multibyte prefix.
    #[test]
    fn find_ascii_case_insensitive_matches_and_yields_boundaries() {
        // Case-insensitive ASCII match.
        assert_eq!(find_ascii_case_insensitive("a <TITLE>x", "<title"), Some(2));
        assert_eq!(find_ascii_case_insensitive("hello", "LO"), Some(3));
        assert_eq!(find_ascii_case_insensitive("hello", "zz"), None);
        assert_eq!(find_ascii_case_insensitive("", "x"), None);
        assert_eq!(find_ascii_case_insensitive("x", ""), None);
        // A multibyte prefix must not produce a mid-character offset: the
        // snowman is 3 bytes, so the tag begins at byte 3, a valid boundary.
        let s = "\u{2603}<title>"; // snowman (3 bytes) + tag
        let pos = find_ascii_case_insensitive(s, "<title").unwrap();
        assert_eq!(pos, 3);
        assert!(s.is_char_boundary(pos));
        assert!(s.is_char_boundary(pos + "<title".len()));
    }

    /// Verifies validate_string_len passes under the cap and errors over it.
    #[test]
    fn validate_string_len_enforces_cap() {
        assert!(validate_string_len("x", "hi", 10).is_ok());
        assert!(validate_string_len("x", "abcdefghijk", 10).is_err());
    }

    /// Verifies clamp_importance_i64 clamps to [MIN_IMPORTANCE, MAX_IMPORTANCE].
    #[test]
    fn clamp_importance_i64_bounds() {
        assert_eq!(clamp_importance_i64(0), 1);
        assert_eq!(clamp_importance_i64(-5), 1);
        assert_eq!(clamp_importance_i64(5), 5);
        assert_eq!(clamp_importance_i64(11), 10);
        assert_eq!(clamp_importance_i64(i64::MAX), 10);
    }

    /// Verifies tag normalization trims/lowercases/drops empties, all-empty
    /// input yields None, and non-JSON-array-of-strings input errors.
    #[test]
    fn normalize_tags_json_normalizes_and_rejects() {
        // Canonical normalization: trim, lowercase, drop empties.
        assert_eq!(
            normalize_tags_json(r#"[" Foo ", "BAR", ""]"#).unwrap(),
            Some(r#"["foo","bar"]"#.to_string())
        );
        // All-empty input stores NULL.
        assert_eq!(normalize_tags_json(r#"["", "  "]"#).unwrap(), None);
        assert_eq!(normalize_tags_json("[]").unwrap(), None);
        // Non-array and non-string-element inputs are rejected.
        assert!(normalize_tags_json("not json").is_err());
        assert!(normalize_tags_json(r#"{"a":1}"#).is_err());
        assert!(normalize_tags_json("[1,2]").is_err());
    }

    /// Verifies validate_batch_size rejects zero and over-cap counts while
    /// accepting the full [1, MAX_BATCH_OPS] range.
    #[test]
    fn validate_batch_size_range() {
        assert!(validate_batch_size(0).is_err());
        assert!(validate_batch_size(1).is_ok());
        assert!(validate_batch_size(MAX_BATCH_OPS).is_ok());
        assert!(validate_batch_size(MAX_BATCH_OPS + 1).is_err());
    }
}
