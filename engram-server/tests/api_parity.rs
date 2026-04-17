//! API Parity Tests
//!
//! Verifies that the Rust engram-server produces response shapes consistent
//! with the TypeScript engram reference implementation.
//! Reference: engram (TypeScript) tests/api.test.mjs (33 tests, 14 suites)
//!
//! Where the Rust implementation diverges from the TS spec, the expected TS
//! shape is documented in a comment above the assertion.

use std::collections::HashMap;
use std::sync::Arc;

use rusqlite::params;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Json;
use axum::Router;
use engram_lib::config::Config;
use engram_lib::db::Database;
use serde_json::{json, Value};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

use engram_lib::cred::CreddClient;
use engram_server::server::build_router;
use engram_server::state::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct TestApp {
    router: Router,
    api_key: String,
    db: Arc<Database>,
}

impl TestApp {
    async fn new() -> Self {
        Self::with_config(Config::default()).await
    }

    async fn with_config(config: Config) -> Self {
        // Ensure auth is enabled regardless of dev environment
        std::env::set_var("ENGRAM_OPEN_ACCESS", "0");
        // /bootstrap now requires a shared secret; set a known test value so
        // the harness can authenticate exactly once to create the admin key.
        std::env::set_var("ENGRAM_BOOTSTRAP_SECRET", "test-bootstrap-secret");
        std::env::set_var("CREDD_AGENT_KEY", "test-agent-key");
        std::env::set_var("CREDD_URL", &config.eidolon.credd.url);
        std::env::set_var(
            "ENGRAM_EIDOLON_SESSIONS_SCRUB_SECRETS",
            if config.eidolon.sessions.scrub_secrets {
                "true"
            } else {
                "false"
            },
        );

        let db = Arc::new(Database::connect_memory().await.expect("in-memory db"));
        let credd = Arc::new(CreddClient::from_config(&config));
        let state = AppState {
            db: Arc::clone(&db),
            config: Arc::new(config),
            credd,
            embedder: Arc::new(RwLock::new(None)),
            reranker: Arc::new(RwLock::new(None)),
            brain: None,
            llm: None,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            eidolon_config: None,
            approval_notify: None,
            pending_approvals: Arc::new(Mutex::new(HashMap::new())),
            safe_mode: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let router = build_router(state);

        // Bootstrap to get an admin API key (public endpoint, secret-gated)
        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/bootstrap")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        serde_json::json!({ "secret": "test-bootstrap-secret" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .expect("bootstrap request failed");

        let body = body_json(res).await;
        let api_key = body["api_key"]
            .as_str()
            .expect("bootstrap did not return api_key")
            .to_string();

        TestApp {
            router,
            api_key,
            db,
        }
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

async fn body_bytes(res: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("failed to read response body")
        .to_vec()
}

async fn pagerank_count_for_user(db: &Database, user_id: i64) -> i64 {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM memory_pagerank WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )
        .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("query pagerank count")
}

async fn distinct_pagerank_users(db: &Database) -> i64 {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(DISTINCT user_id) FROM memory_pagerank",
            [],
            |row| row.get(0),
        )
        .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("query distinct pagerank users")
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

#[tokio::test]
async fn health_ready_returns_200_when_db_ok_and_optional_components_absent() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/health/ready").await;
    assert_eq!(status, StatusCode::OK, "ready should be 200 with live DB");
    assert_eq!(body["status"], "ready");
    assert_eq!(body["checks"]["database"], "ok");
    // Embedder + reranker are optional; their absence must not fail readiness.
    assert_eq!(body["checks"]["embedder"], "disabled");
    assert_eq!(body["checks"]["reranker"], "disabled");
    assert!(body["failing"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn health_live_returns_minimal_payload() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/health/live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

// ---------------------------------------------------------------------------
// AUTH / BOOTSTRAP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_returns_api_key() {
    std::env::set_var("ENGRAM_BOOTSTRAP_SECRET", "test-bootstrap-secret");
    let config = Config::default();
    std::env::set_var("CREDD_AGENT_KEY", "test-agent-key");
    std::env::set_var("CREDD_URL", &config.eidolon.credd.url);
    let db = Database::connect_memory().await.expect("db");
    let credd = Arc::new(CreddClient::from_config(&config));
    let state = AppState {
        db: Arc::new(db),
        config: Arc::new(config),
        credd,
        embedder: Arc::new(RwLock::new(None)),
        reranker: Arc::new(RwLock::new(None)),
        brain: None,
        llm: None,
        sessions: Arc::new(RwLock::new(HashMap::new())),
        eidolon_config: None,
        approval_notify: None,
        pending_approvals: Arc::new(Mutex::new(HashMap::new())),
        safe_mode: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let router = build_router(state);

    let res = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/bootstrap")
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "secret": "test-bootstrap-secret" }).to_string(),
                ))
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
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "secret": "test-bootstrap-secret" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "second bootstrap with valid secret should be 403"
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
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty content should be 400"
    );
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
        .post(
            "/store",
            json!({ "content": "get test", "category": "test" }),
        )
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
        .post(
            "/store",
            json!({ "content": "delete test", "category": "test" }),
        )
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
        .post(
            "/store",
            json!({ "content": "original content", "category": "test" }),
        )
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
    app.post(
        "/store",
        json!({ "content": "list test 1", "category": "test" }),
    )
    .await;
    app.post(
        "/store",
        json!({ "content": "list test 2", "category": "test" }),
    )
    .await;

    let (status, body) = app.get("/list").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    assert!(body["results"].is_array(), "results should be an array");
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
    let tags = tags_body["tags"]
        .as_array()
        .expect("tags should be an array");
    assert!(
        tags.iter()
            .any(|tag| tag["tag"] == "rust" && tag["count"] == 2),
        "expected rust tag count in {tags_body}"
    );

    let (search_status, search_body) = app
        .post("/tags/search", json!({ "tags": ["rust"], "limit": 10 }))
        .await;
    assert!(
        search_status.is_success(),
        "POST /tags/search should succeed"
    );
    let results = search_body["results"]
        .as_array()
        .expect("results should be an array");
    assert_eq!(results.len(), 2, "expected both rust-tagged memories");

    let (update_status, update_body) = app
        .put(
            &format!("/memory/{id}/tags"),
            json!({ "tags": ["updated", "fresh"] }),
        )
        .await;
    assert!(
        update_status.is_success(),
        "PUT /memory/{{id}}/tags should succeed"
    );
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
    assert!(
        body["top_categories"].is_array(),
        "top_categories should be an array"
    );
    assert!(body["top_tags"].is_array(), "top_tags should be an array");
    assert!(
        body.get("personality_traits").is_some(),
        "missing personality_traits"
    );
}

#[tokio::test]
#[ignore = "personality synthesis requires Brain backend or stable rule-based extraction"]
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
    assert!(
        status.is_success(),
        "POST /profile/synthesize should succeed"
    );
    assert!(
        body["personality_summary"].as_str().is_some(),
        "expected synthesized personality summary, got {body}"
    );
}

#[tokio::test]
async fn user_stats_endpoint_returns_user_scoped_counts() {
    let app = TestApp::new().await;
    app.post(
        "/store",
        json!({ "content": "stats memory", "category": "metrics" }),
    )
    .await;

    let (status, body) = app.get("/me/stats").await;
    assert!(status.is_success(), "GET /me/stats should succeed");
    assert!(body["memories"].as_i64().unwrap_or(0) >= 1);
    assert!(
        body["categories"].is_object(),
        "categories should be an object"
    );
}

#[tokio::test]
async fn archive_unarchive_and_forget_endpoints_work() {
    let app = TestApp::new().await;
    let (_status, stored) = app
        .post(
            "/store",
            json!({ "content": "lifecycle memory", "category": "ops" }),
        )
        .await;
    let id = stored["id"].as_i64().unwrap();

    let (archive_status, archive_body) =
        app.post(&format!("/memory/{id}/archive"), json!({})).await;
    assert!(archive_status.is_success(), "archive should succeed");
    assert_eq!(archive_body["status"], "archived");

    let (unarchive_status, unarchive_body) = app
        .post(&format!("/memory/{id}/unarchive"), json!({}))
        .await;
    assert!(unarchive_status.is_success(), "unarchive should succeed");
    assert_eq!(unarchive_body["status"], "active");

    let (forget_status, forget_body) = app
        .post(
            &format!("/memory/{id}/forget"),
            json!({ "reason": "cleanup" }),
        )
        .await;
    assert!(forget_status.is_success(), "forget should succeed");
    assert_eq!(forget_body["status"], "forgotten");

    let (read_status, _read_body) = app.get(&format!("/memory/{id}")).await;
    assert_eq!(
        read_status,
        StatusCode::NOT_FOUND,
        "forgotten memory should not be readable"
    );
}

#[tokio::test]
async fn links_and_versions_endpoints_return_arrays() {
    let app = TestApp::new().await;
    let (_status, stored) = app
        .post(
            "/store",
            json!({ "content": "version root", "category": "test" }),
        )
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
    assert!(
        links_status.is_success(),
        "GET /links/{{id}} should succeed"
    );
    assert!(links_body["links"].is_array(), "links should be an array");

    let (versions_status, versions_body) = app.get(&format!("/versions/{latest_id}")).await;
    assert!(
        versions_status.is_success(),
        "GET /versions/{{id}} should succeed"
    );
    let versions = versions_body["versions"]
        .as_array()
        .expect("versions should be an array");
    assert!(
        versions.len() >= 2,
        "expected version chain, got {versions_body}"
    );
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

#[tokio::test]
async fn schema_endpoints_return_expected_shapes() {
    let app = TestApp::new().await;

    let (status, index) = app.get("/schema").await;
    assert_eq!(status, StatusCode::OK);
    let schemas = index["schemas"].as_array().expect("schemas array");
    let names: Vec<&str> = schemas
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    for n in ["memory", "services", "graph"] {
        assert!(names.contains(&n), "missing schema name: {n}");
    }

    let (status, mem) = app.get("/schema/memory").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mem["name"], "Memory");
    assert!(mem["fields"].as_array().is_some_and(|a| !a.is_empty()));
    assert!(mem["related_shapes"]["SearchResult"]["fields"].is_array());

    let (status, svc) = app.get("/schema/services").await;
    assert_eq!(status, StatusCode::OK);
    let svc_names: Vec<&str> = svc["services"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert!(svc_names.contains(&"axon"));
    assert!(svc_names.contains(&"brain"));

    let (status, graph) = app.get("/schema/graph").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(graph["edge"]["name"], "MemoryLink");
    assert!(graph["endpoints"].as_array().is_some_and(|a| !a.is_empty()));
}

#[tokio::test]
async fn search_explain_returns_score_breakdown_and_timings() {
    let app = TestApp::new().await;
    app.post(
        "/store",
        json!({ "content": "explain endpoint breakdown content", "category": "test" }),
    )
    .await;

    let (status, body) = app
        .post(
            "/search/explain",
            json!({ "query": "explain endpoint breakdown", "limit": 5 }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "expected 200, got {status}: {body}");

    let results = body["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "expected at least one result: {body}");
    let first = &results[0];
    assert!(
        first.get("scores").is_some(),
        "missing scores subobject: {first}"
    );
    let scores = &first["scores"];
    for key in [
        "lexical",
        "vector",
        "graph",
        "personality",
        "temporal_boost",
        "fused",
        "reranked",
        "reranker_ms",
    ] {
        assert!(scores.get(key).is_some(), "missing scores.{key}: {scores}");
    }

    let timings = body["timings_ms"].as_object().expect("timings_ms object");
    for key in ["embed", "hybrid", "rerank", "total"] {
        assert!(
            timings.get(key).and_then(|v| v.as_f64()).is_some(),
            "missing timings_ms.{key}: {body}"
        );
    }
    let pipeline = body["pipeline"].as_object().expect("pipeline object");
    assert!(pipeline.contains_key("embedded"));
    assert!(pipeline.contains_key("reranker_applied"));
}

#[tokio::test]
async fn search_still_works_when_pagerank_job_is_disabled() {
    let config = Config {
        pagerank_enabled: false,
        ..Config::default()
    };
    let app = TestApp::with_config(config).await;

    app.post(
        "/store",
        json!({ "content": "pagerank disabled search smoke test", "category": "test" }),
    )
    .await;

    let (status, body) = app
        .post(
            "/search",
            json!({ "query": "disabled search smoke", "limit": 5 }),
        )
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx"
    );
    let results = body["results"].as_array().expect("results should be array");
    assert!(results.iter().any(|result| {
        result["content"].as_str().unwrap_or("") == "pagerank disabled search smoke test"
    }));
}

#[tokio::test]
async fn admin_pagerank_rebuild_single_user_populates_cache() {
    let app = TestApp::new().await;

    app.post(
        "/store",
        json!({ "content": "admin pagerank single user one", "category": "test" }),
    )
    .await;
    app.post(
        "/store",
        json!({ "content": "admin pagerank single user two", "category": "test" }),
    )
    .await;

    assert_eq!(pagerank_count_for_user(app.db.as_ref(), 1).await, 0);

    let (status, body) = app
        .post("/admin/pagerank/rebuild?user_id=1", json!({}))
        .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}: {body}"
    );
    assert_eq!(body["success"], json!(true));
    assert_eq!(body["users_updated"], json!(1));
    assert!(body["memories_updated"].as_u64().unwrap_or(0) >= 2);
    assert_eq!(pagerank_count_for_user(app.db.as_ref(), 1).await, 2);
}

#[tokio::test]
async fn admin_pagerank_rebuild_all_users_populates_each_users_cache() {
    let app = TestApp::new().await;

    app.post(
        "/store",
        json!({ "content": "admin rebuild user one memory", "category": "test" }),
    )
    .await;

    let user2_key = create_user2_key(&app).await;
    let (status_user2, body_user2) = app
        .post_as(
            "/store",
            json!({ "content": "admin rebuild user two memory", "category": "test" }),
            &user2_key,
        )
        .await;
    assert!(
        status_user2 == StatusCode::OK || status_user2 == StatusCode::CREATED,
        "user 2 store should succeed, got {status_user2}: {body_user2}"
    );

    assert_eq!(distinct_pagerank_users(app.db.as_ref()).await, 0);

    let (status, body) = app.post("/admin/pagerank/rebuild", json!({})).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "expected 2xx, got {status}: {body}"
    );
    assert_eq!(body["success"], json!(true));
    assert!(body["users_updated"].as_u64().unwrap_or(0) >= 2);
    assert_eq!(distinct_pagerank_users(app.db.as_ref()).await, 2);
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

    let (status, body) = app.post("/recall", json!({ "query": "recall test" })).await;
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
async fn admin_cred_resolve_substitutes_secret() {
    let (credd_url, _handle) = spawn_mock_credd().await;
    let mut config = Config::default();
    config.eidolon.credd.url = credd_url;

    let app = TestApp::with_config(config).await;

    let (status, body) = app
        .post(
            "/admin/cred/resolve",
            json!({ "text": "value={{secret:foo/bar}}" }),
        )
        .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["text"], "value=alpha-secret");
}

#[tokio::test]
async fn gate_check_blocks_on_resolved_secret_pattern() {
    let (credd_url, _handle) = spawn_mock_credd().await;
    let mut config = Config::default();
    config.eidolon.credd.url = credd_url;
    config.eidolon.gate.blocked_patterns = vec!["{{secret:security/blocklist/0}}".to_string()];

    let app = TestApp::with_config(config).await;

    let (status, body) = app
        .post(
            "/gate/check",
            json!({
                "command": "curl https://blocked-domain.com",
                "agent": "test-agent"
            }),
        )
        .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["allowed"], false);
    assert!(body["reason"]
        .as_str()
        .unwrap_or_default()
        .contains("blocked pattern"));
}

#[tokio::test]
async fn conversations_scrub_known_credd_secret_before_store() {
    let (credd_url, _handle) = spawn_mock_credd().await;
    let mut config = Config::default();
    config.eidolon.credd.url = credd_url;
    config.eidolon.sessions.scrub_secrets = true;

    let app = TestApp::with_config(config).await;
    let (_status, created) = app
        .post(
            "/conversations",
            json!({ "agent": "test-agent", "title": "Scrub Test" }),
        )
        .await;
    let id = created["id"].as_i64().unwrap();

    let (status, _body) = app
        .post(
            &format!("/conversations/{id}/messages"),
            json!({ "role": "user", "content": "token=alpha-secret" }),
        )
        .await;

    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = app.get(&format!("/conversations/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    let content = body["messages"][0]["content"].as_str().unwrap_or_default();
    assert_eq!(content, "token=[REDACTED]");
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
    assert!(body.get("stored").is_some(), "should have stored field");
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

    let (status, body) = app.get(&format!("/scratch?session={session}")).await;
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

    let (_, listed) = app.get(&format!("/scratch?session={session}")).await;
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
    let (status, _body) = app.get_as(&format!("/memory/{id}"), &user2_key).await;
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
    let (del_status, _) = app.delete_as(&format!("/memory/{id}"), &user2_key).await;
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
    app.post(
        "/store",
        json!({ "content": "user1 only memory", "category": "test" }),
    )
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

async fn spawn_mock_credd() -> (String, tokio::task::JoinHandle<()>) {
    async fn secret_handler(
        axum::extract::Path((service, key)): axum::extract::Path<(String, String)>,
    ) -> Json<Value> {
        let key = key.trim_start_matches('/');
        let payload = match (service.as_str(), key) {
            ("foo", "bar") => json!({
                "service": "foo",
                "key": "bar",
                "type": "ApiKey",
                "value": { "type": "api_key", "key": "alpha-secret" }
            }),
            ("security", "blocklist/0") => json!({
                "service": "security",
                "key": "blocklist/0",
                "type": "Note",
                "value": { "type": "note", "content": "blocked-domain.com" }
            }),
            _ => json!({
                "service": service,
                "key": key,
                "type": "Note",
                "value": { "type": "note", "content": "unknown-secret" }
            }),
        };
        Json(payload)
    }

    async fn list_handler() -> Json<Value> {
        Json(json!({
            "secrets": [
                { "service": "foo", "key": "bar" },
                { "service": "security", "key": "blocklist/0" }
            ]
        }))
    }

    let app = Router::new()
        .route("/secret/{service}/{*key}", get(secret_handler))
        .route("/secrets", get(list_handler));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind credd mock");
    let addr = listener.local_addr().expect("credd mock addr");
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve credd mock");
    });
    (format!("http://{}", addr), handle)
}

// ---------------------------------------------------------------------------
// SSE streaming: /ingest/stream (Part 3.7)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ingest_stream_emits_sse_progress_and_result() {
    let app = TestApp::new().await;

    let req = Request::builder()
        .method("POST")
        .uri("/ingest/stream")
        .header("Authorization", app.bearer())
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .body(Body::from(
            json!({
                "text": "Streaming ingest test body. One paragraph of plaintext content."
            })
            .to_string(),
        ))
        .unwrap();

    let res = app.router.clone().oneshot(req).await.expect("request");
    assert_eq!(res.status(), StatusCode::OK, "expected 200 on stream");
    let content_type = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "expected SSE content-type, got {content_type}"
    );

    let raw = body_bytes(res).await;
    let text = String::from_utf8_lossy(&raw);

    assert!(
        text.contains("event: progress"),
        "expected at least one progress event, body was: {text}"
    );
    assert!(
        text.contains("\"type\":\"detected\"")
            || text.contains("\"type\":\"parsed\"")
            || text.contains("\"type\":\"done\""),
        "expected pipeline phase event in body: {text}"
    );
}

#[tokio::test]
async fn ingest_stream_falls_back_to_json_without_accept_header() {
    let app = TestApp::new().await;

    let req = Request::builder()
        .method("POST")
        .uri("/ingest/stream")
        .header("Authorization", app.bearer())
        .header("Content-Type", "application/json")
        .body(Body::from(json!({ "text": "Fallback body." }).to_string()))
        .unwrap();

    let res = app.router.clone().oneshot(req).await.expect("request");
    assert_eq!(res.status(), StatusCode::OK);
    let raw = body_bytes(res).await;
    let text = String::from_utf8_lossy(&raw);
    // Even without SSE Accept, we frame the result as a single SSE event
    // for consistent client handling.
    assert!(
        text.contains("event: result"),
        "expected result event: {text}"
    );
    assert!(
        text.contains("\"status\":\"completed\"") || text.contains("\"chunks_processed\""),
        "expected ingestion metadata in body: {text}"
    );
}

#[tokio::test]
async fn ingest_stream_rejects_empty_body() {
    let app = TestApp::new().await;

    let req = Request::builder()
        .method("POST")
        .uri("/ingest/stream")
        .header("Authorization", app.bearer())
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .body(Body::from(json!({}).to_string()))
        .unwrap();

    let res = app.router.clone().oneshot(req).await.expect("request");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// 3.8 Resumable chunked upload
// ---------------------------------------------------------------------------

fn sha256_hex_test(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

fn b64_test(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[tokio::test]
async fn upload_init_chunk_complete_end_to_end() {
    let app = TestApp::new().await;

    let (status, init) = app
        .post(
            "/ingest/upload/init",
            json!({
                "filename": "notes.txt",
                "content_type": "text/plain",
                "total_chunks": 3,
                "source": "upload-test",
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "init body: {init}");
    let upload_id = init["upload_id"].as_str().unwrap().to_string();

    let chunks: Vec<&[u8]> = vec![
        b"The quick brown fox ",
        b"jumps over the lazy ",
        b"dog and keeps going.",
    ];
    for (idx, chunk) in chunks.iter().enumerate() {
        let (cs, cb) = app
            .post(
                "/ingest/upload/chunk",
                json!({
                    "upload_id": upload_id,
                    "chunk_index": idx as i64,
                    "chunk_hash": sha256_hex_test(chunk),
                    "data": b64_test(chunk),
                }),
            )
            .await;
        assert_eq!(cs, StatusCode::OK, "chunk {idx} body: {cb}");
        assert_eq!(cb["chunks_received"].as_i64().unwrap(), (idx as i64) + 1);
    }

    // Resuming -- re-upload chunk 1 idempotently.
    let (cs, cb) = app
        .post(
            "/ingest/upload/chunk",
            json!({
                "upload_id": upload_id,
                "chunk_index": 1,
                "chunk_hash": sha256_hex_test(chunks[1]),
                "data": b64_test(chunks[1]),
            }),
        )
        .await;
    assert_eq!(cs, StatusCode::OK, "re-chunk body: {cb}");
    assert_eq!(
        cb["chunks_received"].as_i64().unwrap(),
        3,
        "re-upload should not inflate count"
    );

    let full: Vec<u8> = chunks.concat();
    let (fs, fb) = app
        .post(
            "/ingest/upload/complete",
            json!({
                "upload_id": upload_id,
                "total_chunks": 3,
                "final_sha256": sha256_hex_test(&full),
                "mode": "raw",
            }),
        )
        .await;
    assert_eq!(fs, StatusCode::OK, "complete body: {fb}");
    assert_eq!(fb["status"], "completed");
    assert!(fb["ingested_memories"].as_i64().unwrap() >= 1);
}

#[tokio::test]
async fn upload_chunk_rejects_bad_hash() {
    let app = TestApp::new().await;
    let (_, init) = app.post("/ingest/upload/init", json!({})).await;
    let upload_id = init["upload_id"].as_str().unwrap().to_string();

    let (cs, cb) = app
        .post(
            "/ingest/upload/chunk",
            json!({
                "upload_id": upload_id,
                "chunk_index": 0,
                // Deliberately wrong hash.
                "chunk_hash": "0000000000000000000000000000000000000000000000000000000000000000",
                "data": b64_test(b"hello"),
            }),
        )
        .await;
    assert_eq!(cs, StatusCode::BAD_REQUEST, "body: {cb}");
}

#[tokio::test]
async fn upload_complete_rejects_missing_chunk() {
    let app = TestApp::new().await;
    let (_, init) = app
        .post("/ingest/upload/init", json!({ "total_chunks": 3 }))
        .await;
    let upload_id = init["upload_id"].as_str().unwrap().to_string();

    // Upload chunk 0 and chunk 2, skip 1.
    for idx in [0i64, 2] {
        let data = format!("chunk-{idx}").into_bytes();
        let (cs, _) = app
            .post(
                "/ingest/upload/chunk",
                json!({
                    "upload_id": upload_id,
                    "chunk_index": idx,
                    "chunk_hash": sha256_hex_test(&data),
                    "data": b64_test(&data),
                }),
            )
            .await;
        assert_eq!(cs, StatusCode::OK);
    }

    let (fs, fb) = app
        .post("/ingest/upload/complete", json!({ "upload_id": upload_id }))
        .await;
    assert_eq!(fs, StatusCode::BAD_REQUEST, "body: {fb}");
}

#[tokio::test]
async fn upload_status_reports_received_indices() {
    let app = TestApp::new().await;
    let (_, init) = app.post("/ingest/upload/init", json!({})).await;
    let upload_id = init["upload_id"].as_str().unwrap().to_string();

    for idx in [0i64, 2] {
        let data = format!("part-{idx}").into_bytes();
        app.post(
            "/ingest/upload/chunk",
            json!({
                "upload_id": upload_id,
                "chunk_index": idx,
                "chunk_hash": sha256_hex_test(&data),
                "data": b64_test(&data),
            }),
        )
        .await;
    }

    let (ss, sb) = app.get(&format!("/ingest/upload/{upload_id}/status")).await;
    assert_eq!(ss, StatusCode::OK, "status body: {sb}");
    let received: Vec<i64> = sb["received_indices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    assert_eq!(received, vec![0, 2]);
    assert_eq!(sb["chunks_received"].as_i64().unwrap(), 2);
    assert_eq!(sb["status"], "active");
}

#[tokio::test]
async fn upload_abort_clears_chunks() {
    let app = TestApp::new().await;
    let (_, init) = app.post("/ingest/upload/init", json!({})).await;
    let upload_id = init["upload_id"].as_str().unwrap().to_string();

    let data = b"abort-me".to_vec();
    app.post(
        "/ingest/upload/chunk",
        json!({
            "upload_id": upload_id,
            "chunk_index": 0,
            "chunk_hash": sha256_hex_test(&data),
            "data": b64_test(&data),
        }),
    )
    .await;

    let (abs, abb) = app
        .post("/ingest/upload/abort", json!({ "upload_id": upload_id }))
        .await;
    assert_eq!(abs, StatusCode::OK, "abort body: {abb}");
    assert_eq!(abb["status"], "aborted");

    // A further chunk upload must now fail because the session is not active.
    let (cs, _) = app
        .post(
            "/ingest/upload/chunk",
            json!({
                "upload_id": upload_id,
                "chunk_index": 1,
                "chunk_hash": sha256_hex_test(b"x"),
                "data": b64_test(b"x"),
            }),
        )
        .await;
    assert_eq!(cs, StatusCode::BAD_REQUEST);
}
