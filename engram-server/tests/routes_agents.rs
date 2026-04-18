//! Per-route unit tests for the /agents endpoints.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, delete, get, post, test_app};
use serde_json::json;

// POST /agents happy-path: returns agent_id and name
#[tokio::test]
async fn register_agent_happy_path() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, body) = post(
        &app,
        "/agents",
        &key,
        json!({ "name": "test-agent-alpha", "category": "automation" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "expected 201: {body}");
    assert!(body["agent_id"].as_i64().is_some(), "expected agent_id: {body}");
    assert_eq!(body["name"], "test-agent-alpha");
}

// POST /agents with same name returns 400 (UNIQUE constraint mapped to InvalidInput)
#[tokio::test]
async fn register_agent_duplicate_name_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    post(
        &app,
        "/agents",
        &key,
        json!({ "name": "duplicate-agent" }),
    )
    .await;
    // Second registration with same name
    let (status, _body) = post(
        &app,
        "/agents",
        &key,
        json!({ "name": "duplicate-agent" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "expected 400 on duplicate agent");
}

// GET /agents returns { agents: [...] }
#[tokio::test]
async fn list_agents_returns_agents_array() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    post(&app, "/agents", &key, json!({ "name": "list-agent-one" })).await;
    post(&app, "/agents", &key, json!({ "name": "list-agent-two" })).await;

    let (status, body) = get(&app, "/agents", &key).await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    let agents = body["agents"].as_array().expect("agents array");
    assert!(agents.len() >= 2, "expected at least 2 agents: {body}");
}

// GET /agents/{id} returns agent detail
#[tokio::test]
async fn get_agent_by_id_returns_agent() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (_s, created) = post(
        &app,
        "/agents",
        &key,
        json!({ "name": "detail-agent", "description": "A test agent" }),
    )
    .await;
    let agent_id = created["agent_id"].as_i64().expect("agent_id");

    let (status, body) = get(&app, &format!("/agents/{agent_id}"), &key).await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    assert_eq!(body["id"], agent_id);
    assert_eq!(body["name"], "detail-agent");
}

// GET /agents/{id} for nonexistent id returns 404
#[tokio::test]
async fn get_nonexistent_agent_returns_404() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, _body) = get(&app, "/agents/999999", &key).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// POST /agents without auth returns 401
#[tokio::test]
async fn register_agent_without_auth_returns_401() {
    let (app, _state) = test_app().await;
    let _ = bootstrap_admin_key(&app).await;

    use axum::body::Body;
    use axum::http::Request;
    use common::send;
    let request = Request::builder()
        .method("POST")
        .uri("/agents")
        .header("Content-Type", "application/json")
        .body(Body::from(json!({ "name": "unauth-agent" }).to_string()))
        .unwrap();
    let (status, _body) = send(&app, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// POST /agents with empty name returns 400
#[tokio::test]
async fn register_agent_empty_name_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;
    let (status, _body) = post(&app, "/agents", &key, json!({ "name": "" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
