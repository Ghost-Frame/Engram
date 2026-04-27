//! Stage 1.3 / R3 audit followup #12630 item 3:
//!
//! `/import/bulk`, `/ingest`, and `/ingest/stream` previously returned
//! HTTP 501 NotImplemented when the request had a `url` field instead of
//! inline `text`. There was no planned implementer; the stub created the
//! impression that URL fetching was on the roadmap when it was not.
//!
//! These tests pin the new behaviour: a request that supplies a non-empty
//! `url` is rejected with HTTP 400 BadRequest. Inline-text ingest still
//! works.

mod common;

use axum::http::StatusCode;
use serde_json::json;

use common::{bootstrap_admin_key, post, test_app};

#[tokio::test]
async fn import_bulk_with_url_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, body) = post(
        &app,
        "/import/bulk",
        &key,
        json!({ "url": "https://example.com/file.txt" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "url ingest must be rejected; got status {} body {}",
        status,
        body
    );
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("url ingest is not supported"),
        "expected 'url ingest is not supported' in error, got {:?}",
        err
    );
}

#[tokio::test]
async fn ingest_with_url_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, body) = post(
        &app,
        "/ingest",
        &key,
        json!({ "url": "https://example.com/page" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "url ingest must be rejected; got status {} body {}",
        status,
        body
    );
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("url ingest is not supported"),
        "expected 'url ingest is not supported' in error, got {:?}",
        err
    );
}

#[tokio::test]
async fn ingest_stream_with_url_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, body) = post(
        &app,
        "/ingest/stream",
        &key,
        json!({ "url": "https://example.com/feed" }),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "url ingest must be rejected; got status {} body {}",
        status,
        body
    );
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("url ingest is not supported"),
        "expected 'url ingest is not supported' in error, got {:?}",
        err
    );
}

/// Inline text continues to work. We do not assert on the success body
/// shape (handlers diverge); only that the URL-rejection branch did not
/// eat valid requests.
#[tokio::test]
async fn ingest_inline_text_still_succeeds() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, body) = post(
        &app,
        "/ingest",
        &key,
        json!({ "text": "hello from stage 1.3 regression test" }),
    )
    .await;
    assert!(
        status.is_success(),
        "inline-text /ingest must still succeed; got {} body {}",
        status,
        body
    );
}

/// Empty-string `url` is treated as absent (back-compat for clients that
/// always pass `url: ""` alongside `text`).
#[tokio::test]
async fn ingest_empty_url_with_text_succeeds() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, _body) = post(
        &app,
        "/ingest",
        &key,
        json!({ "url": "", "text": "back-compat: empty url alongside text" }),
    )
    .await;
    assert!(
        status.is_success(),
        "empty url alongside text must still succeed; got {}",
        status
    );
}

/// Neither url nor text -> 400 with the new "text parameter is required"
/// error (replacing the old "Provide url or text parameter").
#[tokio::test]
async fn ingest_with_no_fields_returns_400() {
    let (app, _state) = test_app().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, body) = post(&app, "/ingest", &key, json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let err = body["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("text parameter is required"),
        "expected 'text parameter is required' in error, got {:?}",
        err
    );
}
