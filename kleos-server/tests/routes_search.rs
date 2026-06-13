//! Per-route unit tests for the POST /search (/memories/search) endpoint.
//!
//! # Note on vector search
//! These tests run without an ONNX embedding model. The hybrid search
//! implementation gracefully falls back to lexical-only search when no
//! embedding is available, so all tests pass without model files.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, post, test_app_with_sharding};
use serde_json::json;

// POST /search happy-path: returns { results: [...], abstained, top_score }
#[tokio::test]
async fn search_returns_results_array() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;
    post(
        &app,
        "/store",
        &key,
        json!({ "content": "searchable unit test content", "category": "test" }),
    )
    .await;

    let (status, body) = post(
        &app,
        "/search",
        &key,
        json!({ "query": "unit test content", "limit": 5 }),
    )
    .await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    assert!(body["results"].is_array(), "expected results array: {body}");
    assert!(
        body.get("abstained").is_some(),
        "missing abstained field: {body}"
    );
    assert!(
        body.get("top_score").is_some(),
        "missing top_score field: {body}"
    );
}

// POST /search without auth returns 401
#[tokio::test]
async fn search_without_auth_returns_401() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let _ = bootstrap_admin_key(&app).await;

    use axum::body::Body;
    use axum::http::Request;
    use common::send;
    let request = Request::builder()
        .method("POST")
        .uri("/search")
        .header("Content-Type", "application/json")
        .body(Body::from(json!({ "query": "test" }).to_string()))
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// POST /memories/search is an alias for POST /search and also works
#[tokio::test]
async fn memories_search_alias_works() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, body) = post(
        &app,
        "/memories/search",
        &key,
        json!({ "query": "alias test", "limit": 3 }),
    )
    .await;
    assert!(
        status.is_success(),
        "expected 2xx from alias, got {status}: {body}"
    );
    assert!(body["results"].is_array(), "expected results array: {body}");
}

// POST /search with no stored memories returns empty results (not an error)
#[tokio::test]
async fn search_with_no_memories_returns_empty_results() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, body) = post(
        &app,
        "/search",
        &key,
        json!({ "query": "nothing here", "limit": 5 }),
    )
    .await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    let results = body["results"].as_array().expect("results array");
    assert!(
        results.is_empty(),
        "expected no results for empty DB: {body}"
    );
    assert_eq!(body["abstained"], true);
}

// POST /search with limit capped at 100 (server enforces cap)
#[tokio::test]
async fn search_with_large_limit_is_accepted() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;
    // limit=500 -- server should cap internally and not return an error
    let (status, body) = post(
        &app,
        "/search",
        &key,
        json!({ "query": "anything", "limit": 500 }),
    )
    .await;
    assert!(
        status.is_success(),
        "expected 2xx even for large limit, got {status}: {body}"
    );
}

// POST /search with invalid JSON body returns 422 (deserialization failure)
#[tokio::test]
async fn search_with_bad_json_returns_error() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;

    use axum::body::Body;
    use axum::http::Request;
    use common::send;
    let request = Request::builder()
        .method("POST")
        .uri("/search")
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .body(Body::from("not json at all"))
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422 for bad JSON, got {status}"
    );
}

// GET /decay/scores and POST /decay/refresh are tenant-isolated
#[tokio::test]
async fn decay_scores_isolated_between_tenants() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let admin_key = bootstrap_admin_key(&app).await;
    let (_, alice_key) = common::seed_user(&app, &admin_key, "alice").await;
    let (_, bob_key) = common::seed_user(&app, &admin_key, "bob").await;

    // Alice stores a memory
    post(
        &app,
        "/store",
        &alice_key,
        json!({ "content": "alice memory", "category": "test" }),
    )
    .await;

    // Refresh decay for alice
    let (status, body) = post(&app, "/decay/refresh", &alice_key, json!({})).await;
    assert!(status.is_success(), "alice decay refresh failed: {body}");

    // Bob shouldn't see alice's memories in decay scores
    let (status, body) = common::get(&app, "/decay/scores", &bob_key).await;
    assert!(status.is_success(), "bob decay scores list failed: {body}");
    let memories = body["memories"].as_array().expect("memories array");
    assert!(
        memories.is_empty(),
        "bob should not see alice's decay scores, got {body}"
    );

    // Alice SHOULD see her own
    let (status, body) = common::get(&app, "/decay/scores", &alice_key).await;
    assert!(
        status.is_success(),
        "alice decay scores list failed: {body}"
    );
    let memories = body["memories"].as_array().expect("memories array");
    assert_eq!(
        memories.len(),
        1,
        "alice should see her decay score, got {body}"
    );
}
