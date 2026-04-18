//! Shared test harness for per-route unit tests.
//!
//! # Model loading
//! `test_app()` builds an AppState that intentionally leaves `embedder` and
//! `reranker` as `None`. This means routes that call into the embedding or
//! reranker pipelines will still succeed (the code paths gracefully degrade
//! when the providers are absent) but semantic ranking will not be exercised.
//! Tests that specifically require a loaded ONNX model should be guarded with:
//!
//! ```rust
//! if std::env::var("ENGRAM_TEST_MODEL").is_err() { return; }
//! ```
//!
//! so they are skipped in standard CI where the model files are absent.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, StatusCode};
use axum::Router;
use engram_lib::config::Config;
use engram_lib::db::Database;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

use engram_lib::cred::CreddClient;
use engram_server::server::build_router;
use engram_server::state::AppState;

/// Build a test router, AppState and in-memory SQLite database.
///
/// The returned state has no embedder, reranker, brain, or LLM loaded.
/// Auth is enabled (ENGRAM_OPEN_ACCESS=0) and a bootstrap secret is set.
///
/// The caller receives the raw router (not yet bootstrapped). Use
/// `seed_user` or `bootstrap_admin_key` for an authenticated first user.
pub async fn test_app() -> (Router, AppState) {
    std::env::set_var("ENGRAM_OPEN_ACCESS", "0");
    std::env::set_var("ENGRAM_BOOTSTRAP_SECRET", "test-bootstrap-secret");
    std::env::set_var("CREDD_AGENT_KEY", "test-agent-key");

    let config = Config::default();
    std::env::set_var("CREDD_URL", &config.eidolon.credd.url);

    let db = Arc::new(Database::connect_memory().await.expect("in-memory db"));
    let credd = Arc::new(CreddClient::from_config(&config));
    let state = AppState {
        db,
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
    let router = build_router(state.clone());
    (router, state)
}

/// POST /bootstrap with the test secret and return the admin API key.
pub async fn bootstrap_admin_key(router: &Router) -> String {
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
        .expect("bootstrap request");

    let body = body_json(res).await;
    body["api_key"]
        .as_str()
        .or_else(|| body["key"].as_str())
        .expect("bootstrap did not return api_key")
        .to_string()
}

/// Build an HTTP request. Body defaults to empty.
pub fn req(method: &str, path: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

/// Build an HTTP request with a JSON body.
pub fn req_json(method: &str, path: &str, payload: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header("Content-Type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap()
}

/// Add an Authorization: Bearer <key> header to an existing request builder.
pub fn auth_header(key: &str) -> (HeaderName, HeaderValue) {
    (
        HeaderName::from_static("authorization"),
        HeaderValue::from_str(&format!("Bearer {}", key)).unwrap(),
    )
}

/// Send a request through the router (single-shot) and return status + body.
pub async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let res = app
        .clone()
        .oneshot(request)
        .await
        .expect("request failed");
    let status = res.status();
    let body = body_json(res).await;
    (status, body)
}

/// Convenience: authenticated GET.
pub async fn get(app: &Router, path: &str, key: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(path)
        .header("Authorization", format!("Bearer {}", key))
        .body(Body::empty())
        .unwrap();
    send(app, request).await
}

/// Convenience: authenticated POST with JSON body.
pub async fn post(app: &Router, path: &str, key: &str, payload: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    send(app, request).await
}

/// Convenience: authenticated DELETE.
pub async fn delete(app: &Router, path: &str, key: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("DELETE")
        .uri(path)
        .header("Authorization", format!("Bearer {}", key))
        .body(Body::empty())
        .unwrap();
    send(app, request).await
}

/// Create a new user + write key via the admin API.
/// Returns `(user_id, api_key_string)`.
///
/// Requires an admin-scoped key (`admin_key`).
pub async fn seed_user(
    app: &Router,
    admin_key: &str,
    username: &str,
) -> (i64, String) {
    let (status, body) = post(
        app,
        "/users",
        admin_key,
        serde_json::json!({ "username": username, "role": "writer" }),
    )
    .await;
    assert!(
        status.is_success(),
        "seed_user: create user failed {status}: {body}"
    );
    let user_id = body["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("seed_user: no id in response: {body}"));

    let (kstatus, kbody) = post(
        app,
        "/keys",
        admin_key,
        serde_json::json!({
            "name": format!("{}-key", username),
            "scopes": "read,write",
            "user_id": user_id
        }),
    )
    .await;
    assert!(
        kstatus.is_success(),
        "seed_user: create key failed {kstatus}: {kbody}"
    );
    let api_key = kbody["key"]
        .as_str()
        .unwrap_or_else(|| panic!("seed_user: no key in response: {kbody}"))
        .to_string();

    (user_id, api_key)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

pub async fn body_json(res: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("failed to read response body");
    serde_json::from_slice(&bytes).unwrap_or(serde_json::json!(null))
}
