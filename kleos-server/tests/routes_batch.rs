//! HTTP-level tests for POST /batch stop-on-first-failure semantics.
//!
//! Deep-sweep PR #183 follow-up (F4): the batch route's documented contract is
//! 207 Multi-Status with a results array truncated at the first failing op --
//! including ops that fail insert_link's similarity validation. That contract
//! was documented but untested; these tests pin it.

mod common;

use common::{bootstrap_admin_key, post, test_app_with_sharding};
use serde_json::json;

/// A batch where an invalid-similarity link op fails mid-sequence: the
/// response is 207, results stop at the failing index, prior ops stay
/// committed, and trailing ops are never attempted.
#[tokio::test]
async fn invalid_similarity_link_truncates_batch_with_207() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;

    // Two memories to link.
    let (status, a) = post(
        &app,
        "/memory",
        &key,
        json!({ "content": "batch 207 test source memory" }),
    )
    .await;
    assert!(status.is_success(), "store a: {a}");
    let a_id = a["id"].as_i64().expect("memory id a");
    let (status, b) = post(
        &app,
        "/memory",
        &key,
        json!({ "content": "batch 207 test target memory" }),
    )
    .await;
    assert!(status.is_success(), "store b: {b}");
    let b_id = b["id"].as_i64().expect("memory id b");

    // Op 0 succeeds; op 1 fails insert_link's (0.0, 1.0] similarity guard;
    // op 2 must never run.
    let (status, body) = post(
        &app,
        "/batch",
        &key,
        json!({ "ops": [
            { "op": "store", "body": { "content": "batch op zero survives" } },
            { "op": "link", "body": { "source_id": a_id, "target_id": b_id, "similarity": 42.0 } },
            { "op": "store", "body": { "content": "batch op two must be skipped" } },
        ]}),
    )
    .await;

    assert_eq!(status.as_u16(), 207, "batch response: {body}");
    let results = body["results"].as_array().expect("results array");
    // Truncated at the failing index: op 0 + failed op 1, op 2 omitted.
    assert_eq!(
        results.len(),
        2,
        "results truncated at first failure: {body}"
    );
    assert_eq!(results[0]["success"], true);
    assert_eq!(results[1]["success"], false);
    assert_eq!(results[1]["op"], "link");
    assert_eq!(body["succeeded"], 1);
    assert_eq!(body["total"], 2);

    // Op 0's write stayed committed (non-transactional contract) and op 2's
    // content never landed.
    let (status, search) = post(
        &app,
        "/search",
        &key,
        json!({ "query": "batch op zero survives" }),
    )
    .await;
    assert!(status.is_success(), "search: {search}");
    let hits = search["results"].as_array().expect("search results");
    assert!(
        hits.iter().any(|h| h["content"]
            .as_str()
            .unwrap_or("")
            .contains("op zero survives")),
        "op 0 must stay committed: {search}"
    );
    assert!(
        !hits.iter().any(|h| h["content"]
            .as_str()
            .unwrap_or("")
            .contains("must be skipped")),
        "op 2 must never run: {search}"
    );
}

/// An all-valid batch returns 200 with every op reported successful.
#[tokio::test]
async fn all_valid_batch_returns_200() {
    let (app, _state, _tmp) = test_app_with_sharding().await;
    let key = bootstrap_admin_key(&app).await;

    let (status, body) = post(
        &app,
        "/batch",
        &key,
        json!({ "ops": [
            { "op": "store", "body": { "content": "batch happy path one" } },
            { "op": "store", "body": { "content": "batch happy path two" } },
        ]}),
    )
    .await;

    assert_eq!(status.as_u16(), 200, "batch response: {body}");
    assert_eq!(body["succeeded"], 2);
    assert_eq!(body["total"], 2);
}
