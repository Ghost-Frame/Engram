//! Per-route unit tests for the POST /search (/memories/search) endpoint.
//!
//! # Note on vector search
//! These tests run without an ONNX embedding model. The hybrid search
//! implementation gracefully falls back to lexical-only search when no
//! embedding is available, so all tests pass without model files.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, post, test_app};
use serde_json::json;

// POST /search happy-path: returns { results: [...], abstained, top_score }
#[tokio::test]
async fn search_returns_results_array() {
    let (app, _state) = test_app().await;
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
    assert!(body.get("abstained").is_some(), "missing abstained field: {body}");
    assert!(body.get("top_score").is_some(), "missing top_score field: {body}");
}

// POST /search without auth returns 401
#[tokio::test]
async fn search_without_auth_returns_401() {
    let (app, _state) = test_app().await;
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
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, body) = post(
        &app,
        "/memories/search",
        &key,
        json!({ "query": "alias test", "limit": 3 }),
    )
    .await;
    assert!(status.is_success(), "expected 2xx from alias, got {status}: {body}");
    assert!(body["results"].is_array(), "expected results array: {body}");
}

// POST /search with no stored memories returns empty results (not an error)
#[tokio::test]
async fn search_with_no_memories_returns_empty_results() {
    let (app, _state) = test_app().await;
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
    assert!(results.is_empty(), "expected no results for empty DB: {body}");
    assert_eq!(body["abstained"], true);
}

// POST /search with limit capped at 100 (server enforces cap)
#[tokio::test]
async fn search_with_large_limit_is_accepted() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    // limit=500 -- server should cap internally and not return an error
    let (status, body) = post(
        &app,
        "/search",
        &key,
        json!({ "query": "anything", "limit": 500 }),
    )
    .await;
    assert!(status.is_success(), "expected 2xx even for large limit, got {status}: {body}");
}

// POST /search with invalid JSON body returns 422 (deserialization failure)
#[tokio::test]
async fn search_with_bad_json_returns_error() {
    let (app, _state) = test_app().await;
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
