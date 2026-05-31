//! Integration tests for FSRS auto-update on /recall.

mod common;

use common::{bootstrap_admin_key, get, post, test_app_with_sharding};
use serde_json::json;

// After /recall returns a memory, its FSRS state should be updated
// (access_count incremented, fsrs_reps set, fsrs_stability populated).
#[tokio::test]
async fn recall_updates_fsrs_state() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;

    // Store a memory.
    let (_, stored) = post(
        &app,
        "/store",
        &key,
        json!({ "content": "FSRS recall test memory unique string xq7", "category": "test", "importance": 5 }),
    )
    .await;
    let memory_id = stored["id"].as_i64().unwrap();

    // Recall with a matching query.
    let (status, _) = post(
        &app,
        "/recall",
        &key,
        json!({ "query": "FSRS recall test memory unique string xq7", "limit": 5 }),
    )
    .await;
    assert!(status.is_success(), "recall failed: {status}");

    // Give the background FSRS task time to complete.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Check FSRS state via /fsrs/state?id=X.
    let (status, state) = get(&app, &format!("/fsrs/state?id={memory_id}"), &key).await;
    assert!(status.is_success(), "fsrs/state failed: {status}: {state}");
    assert!(
        state["fsrs_stability"].as_f64().is_some(),
        "fsrs_stability should be set after recall: {state}"
    );
    assert!(
        state["fsrs_reps"].as_i64().unwrap_or(0) >= 1,
        "fsrs_reps should be >= 1 after recall: {state}"
    );
    assert!(
        state["fsrs_last_review_at"].as_str().is_some(),
        "fsrs_last_review_at should be set after recall: {state}"
    );
}
