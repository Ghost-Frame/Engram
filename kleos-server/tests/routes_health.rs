//! Per-route unit tests for the /health family of endpoints.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, get, req, send, test_app};

// GET /health -- unauthenticated, returns { status: "ok", version: ... }
#[tokio::test]
async fn health_returns_200_with_ok_status() {
    let (app, _state) = test_app().await;
    let (status, body) = send(&app, req("GET", "/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body.get("version").is_some(), "missing version field");
}

// GET /health -- no auth required
#[tokio::test]
async fn health_does_not_require_auth() {
    let (app, _state) = test_app().await;
    // No Authorization header at all
    let (status, _body) = send(&app, req("GET", "/health")).await;
    assert!(status.is_success(), "health must not require auth, got {status}");
}

// GET /health/ready -- returns 200 and { status: "ready" } when DB is healthy
#[tokio::test]
async fn health_ready_returns_200_when_db_ok() {
    let (app, _state) = test_app().await;
    let (status, body) = send(&app, req("GET", "/health/ready")).await;
    assert_eq!(status, StatusCode::OK, "/health/ready should be 200 with live DB");
    assert_eq!(body["status"], "ready");
    assert_eq!(body["checks"]["database"], "ok");
}

// GET /health/ready -- embedder/reranker absent must not fail readiness
#[tokio::test]
async fn health_ready_optional_components_absent_still_ready() {
    let (app, _state) = test_app().await;
    let (status, body) = send(&app, req("GET", "/health/ready")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["checks"]["embedder"], "disabled");
    assert_eq!(body["checks"]["reranker"], "disabled");
    assert!(
        body["failing"].as_array().map(|a| a.is_empty()).unwrap_or(true),
        "failing array should be empty when only optional components are absent"
    );
}

// GET /health/live -- returns { status: "ok" }
#[tokio::test]
async fn health_live_returns_200() {
    let (app, _state) = test_app().await;
    let (status, body) = send(&app, req("GET", "/health/live")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

// GET /health after bootstrapping -- still 200
#[tokio::test]
async fn health_returns_200_after_bootstrap() {
    let (app, _state) = test_app().await;
    let _admin_key = bootstrap_admin_key(&app).await;
    let (status, body) = send(&app, req("GET", "/health")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

// Authenticated /health/ready still works (auth is optional on health routes)
#[tokio::test]
async fn health_ready_works_with_auth_header() {
    let (app, _state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;
    let (status, body) = get(&app, "/health/ready", &admin_key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ready");
}
