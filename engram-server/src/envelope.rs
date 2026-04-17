// ============================================================================
// Standard response envelope
// ============================================================================
//
// Uniform JSON shape for list and create endpoints:
//
//     { "data": <T>, "meta": { ... } }   // success
//     { "error": "<msg>", "code": "<opt>" } // error
//
// New handlers should return an `Envelope<T>` (or `ListEnvelope<T>` for lists
// that carry pagination metadata). This gives SDK generators a single,
// predictable shape and keeps error responses aligned with the existing
// `AppError` output (`{error: "..."}`).
//
// Existing handlers that return flat JSON are listed in `engram-lib::pagination`
// under the migration queue; they will be converted incrementally. Until
// then, clients can rely on either shape and new code should prefer the
// envelope.
//
// ## Why not retrofit all routes at once?
//
// A single sweep across ~45 handlers would invalidate every existing SDK.
// The envelope is additive: new routes use it from day one, and legacy
// handlers migrate when touched for unrelated reasons. Tracked in
// `engram-lib::pagination` module docs.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use engram_lib::pagination::PageMeta;
use serde::Serialize;
use serde_json::Value;

/// Success envelope for a single resource or a raw object.
#[derive(Debug, Serialize)]
pub struct Envelope<T: Serialize> {
    pub data: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl<T: Serialize> Envelope<T> {
    /// Wrap a value without meta.
    pub fn new(data: T) -> Self {
        Self { data, meta: None }
    }

    /// Wrap a value with arbitrary JSON meta (e.g. timing, warnings).
    pub fn with_meta(data: T, meta: Value) -> Self {
        Self {
            data,
            meta: Some(meta),
        }
    }
}

impl<T: Serialize> IntoResponse for Envelope<T> {
    fn into_response(self) -> Response {
        (StatusCode::OK, Json(self)).into_response()
    }
}

/// Success envelope for paginated list endpoints. `meta` always carries
/// the `PageMeta` even when empty so clients can parse it unconditionally.
#[derive(Debug, Serialize)]
pub struct ListEnvelope<T: Serialize> {
    pub data: Vec<T>,
    pub meta: PageMeta,
}

impl<T: Serialize> ListEnvelope<T> {
    pub fn new(data: Vec<T>, meta: PageMeta) -> Self {
        Self { data, meta }
    }
}

impl<T: Serialize> IntoResponse for ListEnvelope<T> {
    fn into_response(self) -> Response {
        (StatusCode::OK, Json(self)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn single_envelope_skips_empty_meta() {
        let env = Envelope::new(json!({ "id": 1 }));
        let out = serde_json::to_value(&env).unwrap();
        assert_eq!(out["data"]["id"], 1);
        assert!(out.get("meta").is_none());
    }

    #[test]
    fn single_envelope_preserves_meta() {
        let env = Envelope::with_meta(json!({ "id": 1 }), json!({ "took_ms": 7 }));
        let out = serde_json::to_value(&env).unwrap();
        assert_eq!(out["meta"]["took_ms"], 7);
    }

    #[test]
    fn list_envelope_shape() {
        let rows = vec![json!({ "id": 1 }), json!({ "id": 2 })];
        let meta = PageMeta {
            next_cursor: Some("abc".into()),
            has_more: true,
            total: Some(42),
        };
        let env = ListEnvelope::new(rows, meta);
        let out = serde_json::to_value(&env).unwrap();
        assert_eq!(out["data"].as_array().unwrap().len(), 2);
        assert_eq!(out["meta"]["next_cursor"], "abc");
        assert_eq!(out["meta"]["has_more"], true);
        assert_eq!(out["meta"]["total"], 42);
    }

    #[test]
    fn list_envelope_empty_rows() {
        let rows: Vec<Value> = vec![];
        let meta = PageMeta::default();
        let env = ListEnvelope::new(rows, meta);
        let out = serde_json::to_value(&env).unwrap();
        assert!(out["data"].as_array().unwrap().is_empty());
        assert_eq!(out["meta"]["has_more"], false);
    }
}
