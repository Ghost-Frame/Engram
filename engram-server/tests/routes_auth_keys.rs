//! Per-route unit tests for the /keys endpoint (auth_keys router).
//!
//! The bootstrap admin key has Admin scope, which is required for key management.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, delete, get, post, seed_user, test_app};
use serde_json::json;

// POST /keys happy-path with admin key: returns key, id, name, scopes
#[tokio::test]
async fn create_key_happy_path() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;

    let (status, body) = post(
        &app,
        "/keys",
        &admin_key,
        json!({ "name": "my-test-key", "scopes": "read,write" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "expected 201: {body}");
    assert!(body["key"].as_str().is_some(), "missing key: {body}");
    assert!(body["id"].as_i64().is_some(), "missing id: {body}");
    assert_eq!(body["name"], "my-test-key");
    assert_eq!(body["scopes"], "read,write");
}

// POST /keys without admin scope returns 4xx
#[tokio::test]
async fn create_key_without_admin_scope_returns_4xx() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;
    // Seed a non-admin user (read,write scopes only)
    let (_uid, user_key) = seed_user(&app, &admin_key, "nonadmin-key-user").await;

    let (status, _body) = post(
        &app,
        "/keys",
        &user_key,
        json!({ "name": "attempt", "scopes": "read,write" }),
    )
    .await;
    // Admin scope required -- must be 401 or 403
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "expected 4xx for non-admin, got {status}"
    );
}

// GET /keys lists keys for the authenticated user
#[tokio::test]
async fn list_keys_returns_keys_array() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;
    // Create an extra key so the list has at least 2 entries
    post(
        &app,
        "/keys",
        &admin_key,
        json!({ "name": "second-key", "scopes": "read,write" }),
    )
    .await;

    let (status, body) = get(&app, "/keys", &admin_key).await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    let keys = body["keys"].as_array().expect("keys array");
    assert!(!keys.is_empty(), "expected at least one key: {body}");
}

// DELETE /keys/{id} revokes the key
#[tokio::test]
async fn delete_key_revokes_it() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;
    // Create a key to revoke
    let (_s, created) = post(
        &app,
        "/keys",
        &admin_key,
        json!({ "name": "revocable-key", "scopes": "read,write" }),
    )
    .await;
    let key_id = created["id"].as_i64().expect("key id");

    let (status, body) = delete(&app, &format!("/keys/{key_id}"), &admin_key).await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    assert_eq!(body["revoked"], true);
}

// DELETE /keys/{id} for nonexistent id returns 404
#[tokio::test]
async fn delete_nonexistent_key_returns_404() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;
    let (status, _body) = delete(&app, "/keys/999999", &admin_key).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// POST /keys with admin-scoped caller trying to escalate to admin scope
// (admin already has admin, so this would succeed -- use scope escalation to test 4xx)
// A read,write key cannot grant admin scope it does not hold.
#[tokio::test]
async fn create_key_scope_escalation_rejected() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;
    // Seed a non-admin user
    let (_uid, user_key) = seed_user(&app, &admin_key, "escalation-test-user").await;

    // Non-admin user tries to create a key with admin scope
    let (status, _body) = post(
        &app,
        "/keys",
        &user_key,
        json!({ "name": "escalated-key", "scopes": "admin" }),
    )
    .await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "expected 4xx for scope escalation, got {status}"
    );
}

// GET /keys without auth returns 401
#[tokio::test]
async fn list_keys_without_auth_returns_401() {
    let (app, _state) = test_app().await;
    let _ = bootstrap_admin_key(&app).await;

    use axum::body::Body;
    use axum::http::Request;
    use common::send;
    let request = Request::builder()
        .method("GET")
        .uri("/keys")
        .body(Body::empty())
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
