//! Per-route tests for POST /conversations/{id}/memorize.

mod common;

use common::{bootstrap_admin_key, post, test_app_with_sharding};
use serde_json::json;

// POST /conversations/{id}/memorize on a conversation with no messages
// returns memorized: false.
#[tokio::test]
async fn memorize_empty_conversation_returns_not_memorized() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, conv) = post(
        &app,
        "/conversations",
        &key,
        json!({ "agent": "test-agent" }),
    )
    .await;
    assert!(status.is_success(), "create conversation failed: {conv}");
    let id = conv["id"].as_i64().unwrap();

    let (status, body) = post(
        &app,
        &format!("/conversations/{id}/memorize"),
        &key,
        json!({}),
    )
    .await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    assert_eq!(body["memorized"], false, "expected memorized=false: {body}");
}

// POST /conversations/{id}/memorize stores the transcript as a memory.
#[tokio::test]
async fn memorize_conversation_with_messages_stores_memory() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;

    // Create conversation and add messages.
    let (_, conv) = post(
        &app,
        "/conversations",
        &key,
        json!({ "agent": "test-agent", "title": "Test Session" }),
    )
    .await;
    let id = conv["id"].as_i64().unwrap();

    post(
        &app,
        &format!("/conversations/{id}/messages"),
        &key,
        json!({ "role": "user", "content": "Was ist der Sinn des Lebens?" }),
    )
    .await;
    post(
        &app,
        &format!("/conversations/{id}/messages"),
        &key,
        json!({ "role": "assistant", "content": "42, natürlich." }),
    )
    .await;

    let (status, body) = post(
        &app,
        &format!("/conversations/{id}/memorize"),
        &key,
        json!({}),
    )
    .await;
    assert!(status.is_success(), "expected 2xx, got {status}: {body}");
    assert_eq!(body["memorized"], true, "expected memorized=true: {body}");
    assert!(
        body["memory_id"].as_i64().is_some(),
        "missing memory_id: {body}"
    );
    assert_eq!(body["message_count"], 2, "expected 2 messages: {body}");
}

// POST /conversations/{id}/memorize without auth returns 401.
#[tokio::test]
async fn memorize_without_auth_returns_401() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;

    let (_, conv) = post(
        &app,
        "/conversations",
        &key,
        json!({ "agent": "test-agent" }),
    )
    .await;
    let id = conv["id"].as_i64().unwrap();

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let req = Request::builder()
        .method("POST")
        .uri(format!("/conversations/{id}/memorize"))
        .header("Content-Type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
