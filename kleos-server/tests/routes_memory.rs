//! Per-route unit tests for the memory store / read / delete endpoints.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, delete, get, post, test_app};
use serde_json::json;

// POST /store happy-path: returns stored=true with a numeric id
#[tokio::test]
async fn store_memory_happy_path() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, body) = post(
        &app,
        "/store",
        &key,
        json!({ "content": "unit test memory", "category": "test" }),
    )
    .await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    assert_eq!(body["stored"], true);
    assert!(body["id"].as_i64().is_some(), "expected numeric id: {body}");
}

// POST /store without auth returns 401
#[tokio::test]
async fn store_memory_without_auth_returns_401() {
    let (app, _state) = test_app().await;
    // Bootstrap creates DB but we do NOT use the admin key
    let _ = bootstrap_admin_key(&app).await;

    use axum::body::Body;
    use axum::http::Request;
    use common::send;
    let request = Request::builder()
        .method("POST")
        .uri("/store")
        .header("Content-Type", "application/json")
        .body(Body::from(
            json!({ "content": "no auth", "category": "test" }).to_string(),
        ))
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// POST /store with empty content returns 400
#[tokio::test]
async fn store_memory_empty_content_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, _body) = post(
        &app,
        "/store",
        &key,
        json!({ "content": "", "category": "test" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// POST /store with whitespace-only content returns 400
#[tokio::test]
async fn store_memory_whitespace_content_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, _body) = post(
        &app,
        "/store",
        &key,
        json!({ "content": "   ", "category": "test" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// GET /memory/{id} -- fetch memory stored in previous call
#[tokio::test]
async fn get_memory_by_id_returns_correct_content() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (_s, stored) = post(
        &app,
        "/store",
        &key,
        json!({ "content": "fetchable memory", "category": "test" }),
    )
    .await;
    let id = stored["id"].as_i64().expect("stored id");

    let (status, body) = get(&app, &format!("/memory/{id}"), &key).await;
    assert!(status.is_success(), "expected 2xx, got {status}");
    assert_eq!(body["id"], id);
    assert_eq!(body["content"], "fetchable memory");
}

// GET /memory/{id} for nonexistent id returns 404
#[tokio::test]
async fn get_nonexistent_memory_returns_404() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, _body) = get(&app, "/memory/999999", &key).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// DELETE /memory/{id} returns deleted=true
#[tokio::test]
async fn delete_memory_returns_deleted_true() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (_s, stored) = post(
        &app,
        "/store",
        &key,
        json!({ "content": "to be deleted", "category": "test" }),
    )
    .await;
    let id = stored["id"].as_i64().expect("stored id");

    let (status, body) = delete(&app, &format!("/memory/{id}"), &key).await;
    assert!(status.is_success(), "expected 2xx, got {status}");
    assert_eq!(body["deleted"], true);
}

// DELETE /memory/{id} for nonexistent id returns 404
#[tokio::test]
async fn delete_nonexistent_memory_returns_404() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, _body) = delete(&app, "/memory/999999", &key).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
