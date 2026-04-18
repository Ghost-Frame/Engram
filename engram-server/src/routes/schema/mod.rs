//! Schema introspection endpoints (plan 5.14).
//!
//! Exposes machine-readable type definitions for the core domain objects so
//! SDKs, form generators, and tools like auto-completion can stay in sync
//! with the server without scraping docs or running a full OpenAPI pipeline.
//!
//! Responses are hand-curated (not reflected from Rust types) because:
//!   1. Several response fields are computed, not 1:1 with storage.
//!   2. We want human-friendly `description` strings per field.
//!   3. Adding `schemars` workspace-wide just for this is overkill.
//!
//! When a shape changes on the Rust side, update the corresponding entry
//! here. The `build_*` functions are unit-testable pure data.

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/schema", get(get_index))
        .route("/schema/memory", get(get_memory_schema))
        .route("/schema/services", get(get_services_schema))
        .route("/schema/graph", get(get_graph_schema))
}

async fn get_index() -> Json<Value> {
    Json(json!({
        "schemas": [
            { "name": "memory",   "path": "/schema/memory" },
            { "name": "services", "path": "/schema/services" },
            { "name": "graph",    "path": "/schema/graph" },
        ]
    }))
}

async fn get_memory_schema() -> Json<Value> {
    Json(build_memory_schema())
}

async fn get_services_schema() -> Json<Value> {
    Json(build_services_schema())
}

async fn get_graph_schema() -> Json<Value> {
    Json(build_graph_schema())
}

fn field(name: &str, ty: &str, desc: &str) -> Value {
    json!({ "name": name, "type": ty, "description": desc })
}

fn build_memory_schema() -> Value {
    json!({
        "name": "Memory",
        "description": "A single memory row returned by /memory/{id} and surfaced inside /search and /list results.",
        "fields": [
            field("id",                "i64",              "Stable primary key."),
            field("content",           "string",           "User-visible text payload."),
            field("category",          "string",           "Free-form tag bucket (e.g. 'general', 'code', 'preference')."),
            field("source",            "string",           "Origin of the memory (e.g. 'api', 'cli', 'ingest')."),
            field("session_id",        "string?",          "Optional session grouping key."),
            field("importance",        "i32",              "1-10 scale. Drives decay and recall priority."),
            field("version",           "i32",              "Monotonic version counter for edits."),
            field("is_latest",         "bool",             "True if this row is the current head of its version chain."),
            field("parent_memory_id",  "i64?",             "Previous version id; null for root."),
            field("root_memory_id",    "i64?",             "Head-of-chain id; null for root."),
            field("source_count",      "i32",              "Times this memory was observed/stored."),
            field("is_static",         "bool",             "Pinned -- excluded from decay and forget sweeps."),
            field("is_forgotten",      "bool",             "Soft-deleted via /forget."),
            field("is_archived",       "bool",             "Hidden from default list/search but retained."),
            field("is_fact",           "bool",             "Extracted structured fact."),
            field("is_decomposed",     "bool",             "Has been split into children during decomposition."),
            field("is_superseded",     "bool",             "Replaced by a newer version."),
            field("is_consolidated",   "bool",             "Dream/consolidation pass has merged this row."),
            field("forget_after",      "string?",          "ISO-8601 timestamp; soft-delete scheduled after."),
            field("forget_reason",     "string?",          "Free-form reason recorded when forgotten."),
            field("model",             "string?",          "Embedding model identifier used at write time."),
            field("recall_hits",       "i32",              "Successful retrieval count."),
            field("recall_misses",     "i32",              "Non-matching retrieval attempts."),
            field("adaptive_score",    "f64?",             "Computed adaptive ranking score."),
            field("pagerank_score",    "f64?",             "Personalized PageRank score."),
            field("decay_score",       "f64?",             "Current decay-adjusted importance."),
            field("last_accessed_at",  "string?",          "ISO-8601 last read timestamp."),
            field("access_count",      "i32",              "Total reads since creation."),
            field("tags",              "string[]",         "JSON-array tags."),
            field("episode_id",        "i64?",             "Parent episode id."),
            field("confidence",        "f64",              "0.0-1.0 authoring confidence."),
            field("sync_id",           "string?",          "Cross-device sync identifier."),
            field("status",            "string",           "Lifecycle status (e.g. 'active')."),
            field("user_id",           "i64",              "Owner user id (tenant scope)."),
            field("space_id",          "i64?",             "Optional collaborative space id."),
            field("fsrs_stability",          "f64?", "FSRS stability term."),
            field("fsrs_difficulty",         "f64?", "FSRS difficulty term."),
            field("fsrs_storage_strength",   "f64?", "FSRS storage strength."),
            field("fsrs_retrieval_strength", "f64?", "FSRS retrieval strength."),
            field("fsrs_learning_state",     "i32?", "FSRS learning-stage enum."),
            field("fsrs_reps",               "i32?", "FSRS review count."),
            field("fsrs_lapses",             "i32?", "FSRS lapse count."),
            field("fsrs_last_review_at",     "string?", "ISO-8601 last FSRS review."),
            field("valence",            "f64?",   "Sentiment valence in [-1,1]."),
            field("arousal",            "f64?",   "Sentiment arousal in [0,1]."),
            field("dominant_emotion",   "string?", "Dominant categorical emotion label."),
            field("created_at",         "string",  "ISO-8601 creation timestamp."),
            field("updated_at",         "string",  "ISO-8601 last-modified timestamp."),
        ],
        "related_shapes": {
            "SearchResult": {
                "description": "A memory plus ranking metadata returned by /search and /search/explain.",
                "fields": [
                    field("memory",                    "Memory",   "The matched memory row."),
                    field("score",                     "f64",      "Final fused score used for ordering."),
                    field("search_type",               "string",   "Pipeline stage that surfaced this result."),
                    field("combined_score",            "f64?",     "Pre-rerank fused score (lexical+vector+graph)."),
                    field("semantic_score",            "f64?",     "Vector similarity component."),
                    field("fts_score",                 "f64?",     "FTS5/BM25 lexical component."),
                    field("graph_score",               "f64?",     "Graph/link-based boost."),
                    field("personality_signal_score",  "f64?",     "Personality-signal alignment boost."),
                    field("temporal_boost",            "f64?",     "Recency boost."),
                    field("reranked",                  "bool?",    "True if a reranker reordered this result."),
                    field("reranker_ms",               "f64?",     "Reranker latency in milliseconds."),
                    field("decay_score",               "f64?",     "Decay-adjusted importance at query time."),
                    field("channels",                  "string[]?", "Surface channels that produced this hit."),
                    field("candidate_count",           "usize?",   "Size of the candidate set considered."),
                ],
            }
        }
    })
}

fn build_services_schema() -> Value {
    json!({
        "name": "Services",
        "description": "Auxiliary services exposed under /{service}/* route namespaces. These are optional modules; consult /health/ready to see which are active.",
        "services": [
            {
                "name": "axon",
                "role": "Event bus: publish and subscribe to system-wide events.",
                "example_routes": ["/axon/publish", "/axon/subscribe", "/axon/events"],
            },
            {
                "name": "brain",
                "role": "Dream/consolidation passes: replay, resolve, and strengthen memory links.",
                "example_routes": ["/brain/dream/replay", "/brain/dream/resolve", "/brain/status"],
            },
            {
                "name": "broca",
                "role": "Language / summarization endpoints (optional LLM backend).",
                "example_routes": ["/broca/summarize", "/broca/translate"],
            },
            {
                "name": "chiasm",
                "role": "Cross-session task coordination; owned by the sidecar in this repo.",
                "example_routes": [],
            },
            {
                "name": "loom",
                "role": "Conversation stitching / threading.",
                "example_routes": ["/loom/thread", "/loom/stitch"],
            },
            {
                "name": "soma",
                "role": "Physiological / quota signals (rate-limit pressure, health, load).",
                "example_routes": ["/soma/status"],
            },
            {
                "name": "thymus",
                "role": "Valence / emotional signal pipeline.",
                "example_routes": ["/thymus/score"],
            },
            {
                "name": "intelligence",
                "role": "Fact, preference, and state extraction hooks.",
                "example_routes": ["/intelligence/extract", "/intelligence/reflections"],
            },
            {
                "name": "context",
                "role": "Streaming context assembly (/context, /context/stream).",
                "example_routes": ["/context", "/context/stream"],
            },
            {
                "name": "ingestion",
                "role": "Bulk document ingest (/ingest, /ingest/stream).",
                "example_routes": ["/ingest", "/ingest/stream"],
            },
            {
                "name": "grounding",
                "role": "Citation grounding and source attribution.",
                "example_routes": ["/grounding/cite"],
            },
            {
                "name": "personality",
                "role": "Personality signal synthesis.",
                "example_routes": ["/profile", "/profile/synthesize"],
            },
        ]
    })
}

fn build_graph_schema() -> Value {
    json!({
        "name": "Graph",
        "description": "Memory-link graph: nodes are memories, edges are typed relations stored in memory_links.",
        "node": {
            "type": "Memory",
            "reference": "/schema/memory",
        },
        "edge": {
            "name": "MemoryLink",
            "fields": [
                field("source_id",  "i64",    "Origin memory id."),
                field("target_id",  "i64",    "Destination memory id."),
                field("similarity", "f64",    "Edge weight in [0,1]. Typically cosine similarity or computed score."),
                field("type",       "string", "Link kind: 'similarity', 'causal', 'temporal', 'reference', 'supersedes', 'contradicts', ..."),
                field("created_at", "string", "ISO-8601 creation timestamp."),
            ],
        },
        "endpoints": [
            { "path": "/links/{id}",        "method": "GET",  "desc": "All edges incident on memory {id}." },
            { "path": "/graph/neighbors",   "method": "POST", "desc": "K-hop neighborhood expansion with type filter." },
            { "path": "/graph/communities", "method": "GET",  "desc": "Louvain community assignments." },
            { "path": "/graph/pagerank",    "method": "GET",  "desc": "Per-user personalized PageRank scores." },
        ],
        "link_types": [
            "similarity",
            "causal",
            "temporal",
            "reference",
            "supersedes",
            "contradicts",
            "elaborates",
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_schema_has_expected_top_level_shape() {
        let s = build_memory_schema();
        assert_eq!(s["name"], "Memory");
        assert!(s["fields"].as_array().is_some_and(|a| !a.is_empty()));
        assert!(s["related_shapes"]["SearchResult"]["fields"].is_array());
    }

    #[test]
    fn services_schema_lists_core_services() {
        let s = build_services_schema();
        let names: Vec<&str> = s["services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect();
        for expected in ["axon", "brain", "broca", "loom", "soma", "thymus"] {
            assert!(names.contains(&expected), "missing service: {expected}");
        }
    }

    #[test]
    fn graph_schema_declares_edge_and_endpoints() {
        let s = build_graph_schema();
        assert_eq!(s["edge"]["name"], "MemoryLink");
        assert!(s["endpoints"].as_array().is_some_and(|a| !a.is_empty()));
        assert!(s["link_types"].as_array().is_some_and(|a| !a.is_empty()));
    }
}
