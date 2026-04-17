// ============================================================================
// Pagination standard
// ============================================================================
//
// The canonical pagination model for Engram list endpoints.
//
//   - Request params live in `PageParams { cursor, limit, offset }`.
//   - Response metadata lives in `PageMeta { next_cursor, has_more, total }`.
//
// New list endpoints MUST:
//   1. Accept `PageParams` (via `serde` on a query struct or the provided
//      axum extractor in `engram-server`).
//   2. Clamp `limit` through `PageParams::effective_limit` using the same
//      DEFAULT_SEARCH_LIMIT / MAX_SEARCH_LIMIT ceiling already used by search.
//   3. Return an envelope of `{ data: [...], meta: PageMeta }` so SDK
//      generators see a uniform shape.
//
// Existing routes that still emit `limit / offset` are documented below under
// "legacy offset-based routes" and will be migrated incrementally. Breaking
// their shape in one sweep would invalidate client SDKs; instead they keep
// accepting offset as a fallback and the new cursor field is additive.
//
// ## Cursor encoding
//
// Cursors are opaque base64(url-safe, no pad) strings of the last row key
// the client has seen. A cursor-aware handler decodes the key, issues
// `WHERE id < :cursor_id ORDER BY id DESC LIMIT :limit` (or the reverse on
// ascending feeds), and emits `next_cursor` iff another page exists.
//
// Cursors are stable across inserts (they name a specific row) and survive
// paging across the full history without the classic offset-based duplicate
// / skip artefacts that show up when the underlying set mutates between
// requests.
//
// ## Legacy offset-based routes (migration queue)
//
// These still read `offset`, but now also accept and forward `cursor` where
// the backend supports it. They will be converted to pure cursor pagination
// in a follow-up sweep; the envelope shape is already stable.
//
//   - /tasks, /tasks/stats/feed   (feed already has created_at DESC)
//   - /inbox, /pending
//   - /broca/actions
//   - /conversations/{id}/messages
//   - /sessions
//   - /jobs/pending, /jobs/failed
//   - /audit
//   - /graph/entities
//
// The axon channel cursor is unchanged -- it already uses a monotonic event
// id as the pagination key.

use crate::validation::{DEFAULT_SEARCH_LIMIT, MAX_SEARCH_LIMIT};
use serde::{Deserialize, Serialize};

/// Query-string pagination input. `cursor` is preferred; `offset` remains
/// for legacy routes during migration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PageParams {
    /// Opaque forward cursor. Overrides `offset` when present.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Page size. Clamped to `[1, MAX_SEARCH_LIMIT]`.
    #[serde(default)]
    pub limit: Option<usize>,
    /// Legacy offset-based fallback. Ignored when `cursor` is supplied.
    #[serde(default)]
    pub offset: Option<usize>,
}

impl PageParams {
    /// Return the limit to apply, using the shared search default + cap.
    pub fn effective_limit(&self) -> usize {
        self.limit
            .unwrap_or(DEFAULT_SEARCH_LIMIT)
            .clamp(1, MAX_SEARCH_LIMIT)
    }

    /// Return the offset to apply when the cursor is absent. Cursor-aware
    /// handlers should use `cursor` and ignore this.
    pub fn effective_offset(&self) -> usize {
        if self.cursor.is_some() {
            0
        } else {
            self.offset.unwrap_or(0)
        }
    }

    /// Convenience: borrow the raw cursor value.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }
}

/// Pagination metadata returned alongside list results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PageMeta {
    /// Forward cursor for the next page, or `None` at the end.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    /// True when more rows exist beyond this page.
    pub has_more: bool,
    /// Total match count, if cheap to compute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl PageMeta {
    /// Build a `PageMeta` from a result slice and the limit the caller
    /// asked for. Assumes the caller fetched `limit + 1` rows so it can
    /// detect an extra row cheaply.
    pub fn from_overfetch<T, F>(rows: &mut Vec<T>, limit: usize, cursor_of: F) -> Self
    where
        F: Fn(&T) -> String,
    {
        if rows.len() > limit {
            rows.truncate(limit);
            let next = rows.last().map(&cursor_of);
            Self {
                next_cursor: next,
                has_more: true,
                total: None,
            }
        } else {
            Self {
                next_cursor: None,
                has_more: false,
                total: None,
            }
        }
    }
}

/// Encode a row id into an opaque cursor string.
pub fn encode_cursor(row_id: i64) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(row_id.to_be_bytes())
}

/// Decode a cursor string back to a row id. Returns `None` for malformed
/// input -- callers should treat a bad cursor as "start from the beginning"
/// rather than erroring.
pub fn decode_cursor(cursor: &str) -> Option<i64> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .ok()?;
    if bytes.len() != 8 {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes);
    Some(i64::from_be_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_limit_respects_bounds() {
        assert_eq!(
            PageParams::default().effective_limit(),
            DEFAULT_SEARCH_LIMIT
        );
        assert_eq!(
            PageParams {
                limit: Some(0),
                ..Default::default()
            }
            .effective_limit(),
            1
        );
        assert_eq!(
            PageParams {
                limit: Some(MAX_SEARCH_LIMIT + 500),
                ..Default::default()
            }
            .effective_limit(),
            MAX_SEARCH_LIMIT
        );
    }

    #[test]
    fn cursor_overrides_offset() {
        let p = PageParams {
            cursor: Some("x".into()),
            offset: Some(50),
            ..Default::default()
        };
        assert_eq!(p.effective_offset(), 0);
    }

    #[test]
    fn cursor_roundtrip() {
        let id = 123456789i64;
        let c = encode_cursor(id);
        assert_eq!(decode_cursor(&c), Some(id));
    }

    #[test]
    fn decode_cursor_rejects_garbage() {
        assert_eq!(decode_cursor("!!!!"), None);
        assert_eq!(decode_cursor(""), None);
        assert_eq!(decode_cursor("dGVzdA"), None); // "test" -> 4 bytes, not 8
    }

    #[test]
    fn from_overfetch_emits_cursor_when_extra_row_present() {
        let mut rows: Vec<i64> = vec![10, 9, 8, 7];
        let meta = PageMeta::from_overfetch(&mut rows, 3, |r| encode_cursor(*r));
        assert_eq!(rows.len(), 3);
        assert!(meta.has_more);
        assert_eq!(decode_cursor(meta.next_cursor.as_deref().unwrap()), Some(8));
    }

    #[test]
    fn from_overfetch_no_cursor_when_page_fits() {
        let mut rows: Vec<i64> = vec![10, 9];
        let meta = PageMeta::from_overfetch(&mut rows, 3, |r| encode_cursor(*r));
        assert!(!meta.has_more);
        assert!(meta.next_cursor.is_none());
    }
}
