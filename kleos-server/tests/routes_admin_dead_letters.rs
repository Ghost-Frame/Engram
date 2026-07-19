//! Integration tests for GET /admin/dead-letters.
//!
//! The route exposes the `service_dead_letters` rows that ServiceGuard
//! writes when a guarded internal call exhausts its retries. Before this
//! route existed the table had a writer but no reader, so operators had no
//! way to inspect dead-lettered calls.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, get, seed_user, test_app};

/// Admin callers see seeded dead-letter rows, filtered and shaped as
/// documented; non-admin keys are rejected with 403 by require_admin.
#[tokio::test]
async fn admin_dead_letters_lists_rows_and_gates_on_admin() {
    let (app, state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;

    // Seed one row per service through the production write path.
    kleos_lib::resilience::record_dead_letter(
        &state.db,
        "reranker",
        "rerank",
        serde_json::json!({"query": "q"}),
        "HTTP 503",
        3,
    )
    .await
    .expect("seed reranker dead letter");
    kleos_lib::resilience::record_dead_letter(
        &state.db,
        "embedder",
        "embed",
        serde_json::Value::Null,
        "timeout",
        2,
    )
    .await
    .expect("seed embedder dead letter");

    // Unfiltered listing returns both rows.
    let (status, body) = get(&app, "/admin/dead-letters", &admin_key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 2);
    assert_eq!(body["dead_letters"].as_array().unwrap().len(), 2);

    // Service filter narrows to the matching row.
    let (status, body) = get(&app, "/admin/dead-letters?service=reranker", &admin_key).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["count"], 1);
    assert_eq!(body["dead_letters"][0]["service"], "reranker");
    assert_eq!(body["dead_letters"][0]["error"], "HTTP 503");

    // A key without the admin scope is refused by require_admin at the
    // handler. EngError::Auth maps to 401 (matching every other /admin
    // route), not 403.
    let (_uid, user_key) = seed_user(&app, &admin_key, "deadletter-reader").await;
    let (status, body) = get(&app, "/admin/dead-letters", &user_key).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "body: {body}");
    assert_eq!(body["error"], "admin scope required");
}
