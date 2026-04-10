//! API Parity Tests
//!
//! Verifies that the Rust engram-server produces response shapes consistent
//! with the TypeScript engram reference implementation.
//! Reference: C:\Users\Zan\Projects\engram\tests\api.test.mjs (33 tests, 14 suites)
//!
//! Where the Rust implementation diverges from the TS spec, the expected TS
//! shape is documented in a comment above the assertion.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use engram_lib::config::Config;
use engram_lib::db::Database;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

use engram_server::server::build_router;
use engram_server::state::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct TestApp {
    router: Router,
    api_key: String,
}

impl TestApp {
    async fn new() -> Self {
        let db = Database::connect_memory()
            .await
            .expect("in-memory db");
        let config = Config::default();
        let state = AppState {
            db: Arc::new(db),
            config: Arc::new(config),
            embedder: None,
            reranker: None,
            brain: None,
            llm: None,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            eidolon_config: None,
        };
        let router = build_router(state);

        // Bootstrap to get an admin API key (no auth required on this endpoint)
        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bootstrap")
                    .header("Content-Type", "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("bootstrap request failed");

        let body = body_json(res).await;
        let api_key = body["api_key"]
            .as_str()
            .expect("bootstrap did not return api_key")
            .to_string();

        TestApp { router, api_key }
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let res = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .header("Authorization", self.bearer())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_json(res).await;
        (status, body)
    }

    async fn post(&self, path: &str, payload: Value) -> (StatusCode, Value) {
        let res = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("Authorization", self.bearer())
                    .header("Content-Type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_json(res).await;
        (status, body)
    }

    async fn put(&self, path: &str, payload: Value) -> (StatusCode, Value) {
        let res = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(path)
                    .header("Authorization", self.bearer())
                    .header("Content-Type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_json(res).await;
        (status, body)
    }

    async fn delete(&self, path: &str) -> (StatusCode, Value) {
        let res = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(path)
                    .header("Authorization", self.bearer())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_json(res).await;
        (status, body)
    }

    /// Make a request with a custom Bearer token instead of self.api_key.
    async fn get_as(&self, path: &str, key: &str) -> (StatusCode, Value) {
        let res = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .header("Authorization", format!("Bearer {}", key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_json(res).await;
        (status, body)
    }

    async fn post_as(&self, path: &str, payload: Value, key: &str) -> (StatusCode, Value) {
        let res = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("Authorization", format!("Bearer {}", key))
                    .header("Content-Type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_json(res).await;
        (status, body)
    }

    async fn delete_as(&self, path: &str, key: &str) -> (StatusCode, Value) {
        let res = self
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(path)
                    .header("Authorization", format!("Bearer {}", key))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = body_json(res).await;
        (status, body)
    }
}

async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).unwrap_or(json!(null))
}

// ---------------------------------------------------------------------------
// HEALTH
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_ok_status() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/health").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}"
    );
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn health_returns_version_field() {
    let app = TestApp::new().await;
    let (_status, body) = app.get("/health").await;
    assert!(
        body.get("version").is_some(),
        "response should include version field"
    );
}

// ---------------------------------------------------------------------------
// AUTH / BOOTSTRAP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_returns_api_key() {
    let db = Database::connect_memory().await.expect("db");
    let state = AppState {
        db: Arc::new(db),
        config: Arc::new(Config::default()),
        embedder: None,
        reranker: None,
        brain: None,
        llm: None,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        eidolon_config: None,
    };
    let router = build_router(state);

    let res = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/bootstrap")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = res.status();
    let body = body_json(res).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}"
    );
    assert!(
        body.get("api_key").and_then(|v| v.as_str()).is_some(),
        "bootstrap should return api_key"
    );
    assert!(
        body.get("key").and_then(|v| v.as_str()).is_some(),
        "bootstrap should return key"
    );
}

#[tokio::test]
async fn bootstrap_is_idempotent_returns_forbidden() {
    let app = TestApp::new().await; // already bootstrapped
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/bootstrap")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "second bootstrap should be 403"
    );
}

#[tokio::test]
async fn unauthenticated_request_returns_401() {
    let app = TestApp::new().await;
    let res = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/list")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// MEMORY -- STORE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn store_memory_returns_stored_true_with_id() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post(
            "/store",
            json!({ "content": "parity test memory", "category": "test", "importance": 7 }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}"
    );
    assert_eq!(body["stored"], true, "stored should be true");
    assert!(
        body.get("id").and_then(|v| v.as_i64()).is_some(),
        "response should include numeric id"
    );
}

#[tokio::test]
async fn store_memory_response_shape() {
    let app = TestApp::new().await;
    let (_status, body) = app
        .post(
            "/store",
            json!({ "content": "shape test memory", "category": "test" }),
        )
        .await;
    // Verify all expected fields are present
    assert!(body.get("id").is_some(), "missing id");
    assert!(body.get("stored").is_some(), "missing stored");
    assert!(body.get("created_at").is_some(), "missing created_at");
    assert!(body.get("importance").is_some(), "missing importance");
    assert!(body.get("tags").is_some(), "missing tags");
}

#[tokio::test]
async fn store_empty_content_returns_400() {
    let app = TestApp::new().await;
    let (status, _body) = app
        .post("/store", json!({ "content": "", "category": "test" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "empty content should be 400");
}

#[tokio::test]
async fn store_whitespace_only_returns_400() {
    let app = TestApp::new().await;
    let (status, _body) = app
        .post("/store", json!({ "content": "   ", "category": "test" }))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "whitespace-only content should be 400"
    );
}

// ---------------------------------------------------------------------------
// MEMORY -- GET / DELETE / UPDATE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_memory_by_id_returns_correct_id() {
    let app = TestApp::new().await;
    let (_s, stored) = app
        .post("/store", json!({ "content": "get test", "category": "test" }))
        .await;
    let id = stored["id"].as_i64().unwrap();

    let (status, body) = app.get(&format!("/memory/{id}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["id"], id, "returned wrong memory id");
    assert_eq!(body["content"], "get test");
}

#[tokio::test]
async fn get_nonexistent_memory_returns_404() {
    let app = TestApp::new().await;
    let (status, _body) = app.get("/memory/999999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_memory_returns_deleted_true() {
    let app = TestApp::new().await;
    let (_s, stored) = app
        .post("/store", json!({ "content": "delete test", "category": "test" }))
        .await;
    let id = stored["id"].as_i64().unwrap();

    let (status, body) = app.delete(&format!("/memory/{id}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["deleted"], true);
}

#[tokio::test]
async fn update_memory_returns_updated_memory() {
    let app = TestApp::new().await;
    let (_s, stored) = app
        .post("/store", json!({ "content": "original content", "category": "test" }))
        .await;
    let id = stored["id"].as_i64().unwrap();

    // TS expects: { new_id, version >= 2 }
    // Rust returns: full memory_to_json object with id and version fields
    let (status, body) = app
        .post(
            &format!("/memory/{id}/update"),
            json!({ "content": "updated content v2" }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    // Rust returns the full memory, not { new_id, version }
    assert!(
        body.get("id").is_some() || body.get("new_id").is_some(),
        "update should return id or new_id"
    );
}

#[tokio::test]
async fn list_memories_returns_results_array() {
    let app = TestApp::new().await;
    app.post("/store", json!({ "content": "list test 1", "category": "test" }))
        .await;
    app.post("/store", json!({ "content": "list test 2", "category": "test" }))
        .await;

    let (status, body) = app.get("/list").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert!(
        body["results"].is_array(),
        "results should be an array"
    );
}

#[tokio::test]
async fn tags_endpoints_list_search_and_update() {
    let app = TestApp::new().await;
    let (_status, stored) = app
        .post(
            "/store",
            json!({ "content": "tagged memory one", "category": "test", "tags": ["Rust", "Systems"] }),
        )
        .await;
    let id = stored["id"].as_i64().unwrap();

    app.post(
        "/store",
        json!({ "content": "tagged memory two", "category": "test", "tags": ["rust", "Backend"] }),
    )
    .await;

    let (tags_status, tags_body) = app.get("/tags").await;
    assert!(tags_status.is_success(), "GET /tags should succeed");
    let tags = tags_body["tags"].as_array().expect("tags should be an array");
    assert!(
        tags.iter().any(|tag| tag["tag"] == "rust" && tag["count"] == 2),
        "expected rust tag count in {tags_body}"
    );

    let (search_status, search_body) = app
        .post("/tags/search", json!({ "tags": ["rust"], "limit": 10 }))
        .await;
    assert!(search_status.is_success(), "POST /tags/search should succeed");
    let results = search_body["results"].as_array().expect("results should be an array");
    assert_eq!(results.len(), 2, "expected both rust-tagged memories");

    let (update_status, update_body) = app
        .put(&format!("/memory/{id}/tags"), json!({ "tags": ["updated", "fresh"] }))
        .await;
    assert!(update_status.is_success(), "PUT /memory/{{id}}/tags should succeed");
    assert_eq!(update_body["tags"], json!(["updated", "fresh"]));
}

#[tokio::test]
async fn profile_endpoint_returns_combined_profile_shape() {
    let app = TestApp::new().await;
    app.post(
        "/store",
        json!({ "content": "profile memory", "category": "journal", "tags": ["alpha"] }),
    )
    .await;

    let (status, body) = app.get("/profile").await;
    assert!(status.is_success(), "GET /profile should succeed");
    assert!(body.get("user_id").and_then(|v| v.as_i64()).is_some());
    assert!(body.get("memory_count").and_then(|v| v.as_i64()).is_some());
    assert!(body["top_categories"].is_array(), "top_categories should be an array");
    assert!(body["top_tags"].is_array(), "top_tags should be an array");
    assert!(body.get("personality_traits").is_some(), "missing personality_traits");
}

#[tokio::test]
async fn profile_synthesize_returns_summary() {
    let app = TestApp::new().await;
    app.post(
        "/store",
        json!({
            "content": "I love building Rust systems, I value clarity in code, and I want to learn distributed systems because it matters deeply to me.",
            "category": "journal"
        }),
    )
    .await;

    let (status, body) = app.post("/profile/synthesize", json!({})).await;
    assert!(status.is_success(), "POST /profile/synthesize should succeed");
    assert!(
        body["personality_summary"].as_str().is_some(),
        "expected synthesized personality summary, got {body}"
    );
}

#[tokio::test]
async fn user_stats_endpoint_returns_user_scoped_counts() {
    let app = TestApp::new().await;
    app.post("/store", json!({ "content": "stats memory", "category": "metrics" }))
        .await;

    let (status, body) = app.get("/me/stats").await;
    assert!(status.is_success(), "GET /me/stats should succeed");
    assert!(body["memories"].as_i64().unwrap_or(0) >= 1);
    assert!(body["categories"].is_object(), "categories should be an object");
}

#[tokio::test]
async fn archive_unarchive_and_forget_endpoints_work() {
    let app = TestApp::new().await;
    let (_status, stored) = app
        .post("/store", json!({ "content": "lifecycle memory", "category": "ops" }))
        .await;
    let id = stored["id"].as_i64().unwrap();

    let (archive_status, archive_body) = app.post(&format!("/memory/{id}/archive"), json!({})).await;
    assert!(archive_status.is_success(), "archive should succeed");
    assert_eq!(archive_body["status"], "archived");

    let (unarchive_status, unarchive_body) = app.post(&format!("/memory/{id}/unarchive"), json!({})).await;
    assert!(unarchive_status.is_success(), "unarchive should succeed");
    assert_eq!(unarchive_body["status"], "active");

    let (forget_status, forget_body) = app
        .post(&format!("/memory/{id}/forget"), json!({ "reason": "cleanup" }))
        .await;
    assert!(forget_status.is_success(), "forget should succeed");
    assert_eq!(forget_body["status"], "forgotten");

    let (read_status, _read_body) = app.get(&format!("/memory/{id}")).await;
    assert_eq!(read_status, StatusCode::NOT_FOUND, "forgotten memory should not be readable");
}

#[tokio::test]
async fn links_and_versions_endpoints_return_arrays() {
    let app = TestApp::new().await;
    let (_status, stored) = app
        .post("/store", json!({ "content": "version root", "category": "test" }))
        .await;
    let id = stored["id"].as_i64().unwrap();

    let (update_status, updated) = app
        .post(
            &format!("/memory/{id}/update"),
            json!({ "content": "version root updated" }),
        )
        .await;
    assert!(update_status.is_success(), "update should succeed");
    let latest_id = updated["id"].as_i64().unwrap();

    let (links_status, links_body) = app.get(&format!("/links/{latest_id}")).await;
    assert!(links_status.is_success(), "GET /links/{{id}} should succeed");
    assert!(links_body["links"].is_array(), "links should be an array");

    let (versions_status, versions_body) = app.get(&format!("/versions/{latest_id}")).await;
    assert!(versions_status.is_success(), "GET /versions/{{id}} should succeed");
    let versions = versions_body["versions"].as_array().expect("versions should be an array");
    assert!(versions.len() >= 2, "expected version chain, got {versions_body}");
}

// ---------------------------------------------------------------------------
// SEARCH
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_returns_results_array() {
    let app = TestApp::new().await;
    app.post(
        "/store",
        json!({ "content": "searchable content for parity test", "category": "test" }),
    )
    .await;

    let (status, body) = app
        .post("/search", json!({ "query": "parity test", "limit": 5 }))
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert!(body["results"].is_array(), "results should be an array");
}

#[tokio::test]
async fn search_response_shape() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post("/search", json!({ "query": "anything", "limit": 3 }))
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert!(body.get("results").is_some(), "missing results");
    assert!(body.get("abstained").is_some(), "missing abstained");
    assert!(body.get("top_score").is_some(), "missing top_score");
}

// ---------------------------------------------------------------------------
// RECALL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recall_returns_memories_array() {
    let app = TestApp::new().await;
    app.post(
        "/store",
        json!({ "content": "recall test memory", "category": "test", "importance": 8 }),
    )
    .await;

    let (status, body) = app
        .post("/recall", json!({ "query": "recall test" }))
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    // Rust returns { memories, breakdown, count }
    // TS test checks: data.memories || data.results
    assert!(
        body["memories"].is_array() || body["results"].is_array(),
        "recall should return memories or results array"
    );
}

// ---------------------------------------------------------------------------
// CONVERSATIONS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_conversation_returns_id() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post(
            "/conversations",
            json!({ "agent": "test-agent", "title": "Test Conversation" }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}"
    );
    assert!(
        body.get("id").and_then(|v| v.as_i64()).is_some(),
        "should return id"
    );
}

#[tokio::test]
async fn list_conversations_returns_array() {
    let app = TestApp::new().await;
    app.post(
        "/conversations",
        json!({ "agent": "test-agent", "title": "List Test" }),
    )
    .await;

    let (status, body) = app.get("/conversations").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    // NOTE: TS expects { results: [...] } -- Rust returns { conversations: [...] }
    assert!(
        body["conversations"].is_array() || body["results"].is_array(),
        "should return conversations or results array"
    );
}

#[tokio::test]
async fn get_conversation_by_id() {
    let app = TestApp::new().await;
    let (_s, created) = app
        .post(
            "/conversations",
            json!({ "agent": "test-agent", "title": "Get Test" }),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, body) = app.get(&format!("/conversations/{id}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    // NOTE: TS expects { conversation: { user_id: 1, ... } }
    // Rust returns the fields directly (id, agent, title, ...)
    assert!(
        body.get("id").is_some() || body.get("conversation").is_some(),
        "should return id or conversation object"
    );
}

#[tokio::test]
async fn add_message_to_conversation() {
    let app = TestApp::new().await;
    let (_s, created) = app
        .post(
            "/conversations",
            json!({ "agent": "test-agent", "title": "Msg Test" }),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, _body) = app
        .post(
            &format!("/conversations/{id}/messages"),
            json!({ "role": "user", "content": "hello from test" }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx adding message"
    );
}

#[tokio::test]
async fn delete_conversation_returns_deleted_true() {
    let app = TestApp::new().await;
    let (_s, created) = app
        .post(
            "/conversations",
            json!({ "agent": "test-agent", "title": "Delete Test" }),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, body) = app.delete(&format!("/conversations/{id}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["deleted"], true);
}

#[tokio::test]
async fn bulk_insert_conversation_returns_id_and_message_count() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post(
            "/conversations/bulk",
            json!({
                "agent": "test-bulk",
                "messages": [
                    { "role": "user", "content": "hello" },
                    { "role": "assistant", "content": "hi there" }
                ]
            }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}: {body}"
    );
    assert!(
        body.get("id").and_then(|v| v.as_i64()).is_some(),
        "bulk insert should return id"
    );
}

// ---------------------------------------------------------------------------
// SCRATCHPAD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scratchpad_put_stores_entries_and_returns_session() {
    let app = TestApp::new().await;
    let session = "test-session-parity-put";
    let (status, body) = app
        .put(
            "/scratch",
            json!({
                "session": session,
                "agent": "test-agent",
                "model": "test-model",
                "entries": [
                    { "key": "task:test-1", "value": "value one" },
                    { "key": "task:test-2", "value": "value two" }
                ]
            }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}"
    );
    assert_eq!(body["session"], session, "should echo back session");
    // NOTE: TS expects stored === true and count >= 2
    // Rust returns stored as a count (number) and ttl_minutes instead of count
    assert!(
        body.get("stored").is_some(),
        "should have stored field"
    );
}

#[tokio::test]
async fn scratchpad_get_returns_entries_array() {
    let app = TestApp::new().await;
    let session = "test-session-parity-get";
    app.put(
        "/scratch",
        json!({
            "session": session,
            "agent": "test-agent",
            "entries": [{ "key": "task:list-me", "value": "listed" }]
        }),
    )
    .await;

    let (status, body) = app
        .get(&format!("/scratch?session={session}"))
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert!(body["entries"].is_array(), "entries should be an array");
    let entries = body["entries"].as_array().unwrap();
    assert!(
        entries.iter().any(|e| e["key"] == "task:list-me"),
        "stored entry should appear in list"
    );
}

#[tokio::test]
async fn scratchpad_delete_key_removes_entry() {
    let app = TestApp::new().await;
    let session = "test-session-parity-del-key";
    app.put(
        "/scratch",
        json!({
            "session": session,
            "agent": "test-agent",
            "entries": [
                { "key": "keep:this", "value": "keep" },
                { "key": "delete:this", "value": "gone" }
            ]
        }),
    )
    .await;

    let (status, body) = app
        .delete(&format!("/scratch/{session}/delete%3Athis"))
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["deleted"], true);
    // NOTE: TS checks data.key === "editing:routes" -- Rust returns the key that was deleted
    assert!(body.get("key").is_some(), "should return deleted key");
}

#[tokio::test]
async fn scratchpad_delete_session_removes_all_entries() {
    let app = TestApp::new().await;
    let session = "test-session-parity-del-session";
    app.put(
        "/scratch",
        json!({
            "session": session,
            "agent": "test-agent",
            "entries": [
                { "key": "a", "value": "1" },
                { "key": "b", "value": "2" }
            ]
        }),
    )
    .await;

    let (status, body) = app.delete(&format!("/scratch/{session}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["deleted"], true);

    let (_, listed) = app
        .get(&format!("/scratch?session={session}"))
        .await;
    assert_eq!(
        listed["count"].as_i64().unwrap_or(0),
        0,
        "session should have no entries after delete"
    );
}

// ---------------------------------------------------------------------------
// GRAPH -- ENTITIES
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_entity_returns_id() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post(
            "/entities",
            json!({ "name": "TestEntity", "entity_type": "tool", "description": "parity test entity" }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}: {body}"
    );
    assert!(
        body.get("id").and_then(|v| v.as_i64()).is_some(),
        "should return numeric id"
    );
}

#[tokio::test]
async fn get_entity_by_id_returns_entity() {
    let app = TestApp::new().await;
    let (_s, created) = app
        .post(
            "/entities",
            json!({ "name": "GetTestEntity", "entity_type": "concept" }),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, body) = app.get(&format!("/entities/{id}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["name"], "GetTestEntity");
    assert_eq!(body["id"], id);
}

#[tokio::test]
async fn delete_entity_returns_deleted_true() {
    let app = TestApp::new().await;
    let (_s, created) = app
        .post(
            "/entities",
            json!({ "name": "DeleteTestEntity", "entity_type": "tool" }),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, body) = app.delete(&format!("/entities/{id}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["deleted"], true);
}

// ---------------------------------------------------------------------------
// FSRS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fsrs_state_has_stability_key() {
    let app = TestApp::new().await;
    let (_s, stored) = app
        .post(
            "/store",
            json!({ "content": "fsrs state test", "category": "test", "importance": 5 }),
        )
        .await;
    let id = stored["id"].as_i64().unwrap();

    let (status, body) = app.get(&format!("/fsrs/state?id={id}")).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert!(
        body.get("fsrs_stability").is_some(),
        "response should include fsrs_stability key"
    );
}

#[tokio::test]
async fn fsrs_review_records_review_and_returns_id() {
    let app = TestApp::new().await;
    let (_s, stored) = app
        .post(
            "/store",
            json!({ "content": "fsrs review test", "category": "test" }),
        )
        .await;
    let id = stored["id"].as_i64().unwrap();

    let (status, body) = app
        .post("/fsrs/review", json!({ "id": id, "grade": 3 }))
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert_eq!(body["id"], id, "review should echo back the memory id");
}

// ---------------------------------------------------------------------------
// MULTI-TENANT ISOLATION
// ---------------------------------------------------------------------------
//
// These tests verify that user A cannot access user B's data through any
// endpoint. This validates the tenant-isolation hardening work.

/// Create a second user and return a write-scoped API key for them.
/// The bootstrap DB only has user_id=1; user_id=2 must be created first.
async fn create_user2_key(app: &TestApp) -> String {
    // Create user 2 via POST /users (requires admin scope)
    let (user_status, user_body) = app
        .post(
            "/users",
            json!({ "username": "user2-isolation-test", "role": "writer" }),
        )
        .await;
    assert!(
        user_status.is_success(),
        "failed to create user 2: {user_status}: {user_body}"
    );
    let user2_id = user_body["id"].as_i64().unwrap_or(2);

    // Create a write key for user 2
    let (key_status, key_body) = app
        .post(
            "/keys",
            json!({
                "name": "user2-test-key",
                "scopes": "read,write",
                "user_id": user2_id
            }),
        )
        .await;
    key_body["key"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("create key should return key field, got {key_status}: {key_body}")
        })
        .to_string()
}

#[tokio::test]
async fn multi_tenant_user_b_cannot_read_user_a_memory() {
    let app = TestApp::new().await;

    // User 1 stores a memory
    let (_s, stored) = app
        .post(
            "/store",
            json!({ "content": "secret memory for user 1", "category": "private" }),
        )
        .await;
    let id = stored["id"].as_i64().unwrap();

    // Create key for user 2
    let user2_key = create_user2_key(&app).await;

    // User 2 tries to read user 1's memory
    let (status, _body) = app
        .get_as(&format!("/memory/{id}"), &user2_key)
        .await;
    // Should be 404 (not found for this user) or 401
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::UNAUTHORIZED,
        "user B should not be able to read user A's memory, got {status}"
    );
}

#[tokio::test]
async fn multi_tenant_user_b_cannot_delete_user_a_memory() {
    let app = TestApp::new().await;

    // User 1 stores a memory
    let (_s, stored) = app
        .post(
            "/store",
            json!({ "content": "isolation delete test", "category": "private" }),
        )
        .await;
    let id = stored["id"].as_i64().unwrap();

    // Create key for user 2
    let user2_key = create_user2_key(&app).await;

    // User 2 tries to delete user 1's memory
    let (del_status, _) = app
        .delete_as(&format!("/memory/{id}"), &user2_key)
        .await;
    // Should fail (404 since delete_memory doesn't scope by user, but the memory won't be "found" in practice)
    // After the attempted delete, verify user 1 can still read it
    let (read_status, read_body) = app.get(&format!("/memory/{id}")).await;
    assert!(
        read_status == StatusCode::OK || read_status == StatusCode::CREATED,
        "user A's memory should still be readable after user B's delete attempt, got {read_status}: {read_body}"
    );
    let _ = del_status; // silence unused warning
}

#[tokio::test]
async fn multi_tenant_search_is_scoped_to_user() {
    let app = TestApp::new().await;

    // User 1 stores a unique memory
    let unique_content = "xk9_isolation_marker_unique_sentinel_99z";
    app.post(
        "/store",
        json!({ "content": unique_content, "category": "test" }),
    )
    .await;

    // Create key for user 2
    let user2_key = create_user2_key(&app).await;

    // User 2 searches -- should NOT find user 1's memory
    let (status, body) = app
        .post_as(
            "/search",
            json!({ "query": "isolation_marker_unique_sentinel", "limit": 10 }),
            &user2_key,
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "search should succeed for user 2"
    );
    let results = body["results"].as_array().expect("results should be array");
    let found_cross_tenant = results
        .iter()
        .any(|r| r["content"].as_str().unwrap_or("") == unique_content);
    assert!(
        !found_cross_tenant,
        "user B should not see user A's memories in search results"
    );
}

#[tokio::test]
async fn multi_tenant_list_is_scoped_to_user() {
    let app = TestApp::new().await;

    // User 1 stores a memory
    app.post("/store", json!({ "content": "user1 only memory", "category": "test" }))
        .await;

    // Create key for user 2
    let user2_key = create_user2_key(&app).await;

    // User 2 lists memories
    let (status, body) = app.get_as("/list", &user2_key).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "list should succeed for user 2"
    );
    let results = body["results"].as_array().expect("results should be array");
    let cross_tenant = results
        .iter()
        .any(|r| r["content"].as_str().unwrap_or("") == "user1 only memory");
    assert!(
        !cross_tenant,
        "user B list should not include user A's memories"
    );
}

#[tokio::test]
async fn multi_tenant_tags_are_scoped_to_user() {
    let app = TestApp::new().await;

    app.post(
        "/store",
        json!({ "content": "tag isolation memory", "category": "test", "tags": ["tenant-a-only"] }),
    )
    .await;

    let user2_key = create_user2_key(&app).await;

    let (status, body) = app.get_as("/tags", &user2_key).await;
    assert!(status.is_success(), "user 2 tags request should succeed");
    let tags = body["tags"].as_array().expect("tags should be an array");
    assert!(
        !tags.iter().any(|tag| tag["tag"] == "tenant-a-only"),
        "user B should not see user A tags, got {body}"
    );
}

#[tokio::test]
async fn multi_tenant_conversations_are_scoped() {
    let app = TestApp::new().await;

    // User 1 creates a conversation
    let (_s, created) = app
        .post(
            "/conversations",
            json!({ "agent": "isolation-agent", "title": "User1 Private Conv" }),
        )
        .await;
    let conv_id = created["id"].as_i64().unwrap();

    // Create key for user 2
    let user2_key = create_user2_key(&app).await;

    // User 2 tries to access user 1's conversation
    let (status, _body) = app
        .get_as(&format!("/conversations/{conv_id}"), &user2_key)
        .await;
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::UNAUTHORIZED,
        "user B should not be able to access user A's conversation, got {status}"
    );
}
