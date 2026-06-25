//! Integration test for POST /admin/reset tenant scoping.
//!
//! In monolith mode the reset must wipe only the calling admin's own rows,
//! not every tenant's data. Before the fix the DELETEs carried no predicate
//! and erased all tenants' memories on the shared DB.

mod common;

use axum::http::StatusCode;
use common::{bootstrap_admin_key, post, test_app};
use serde_json::json;

/// Insert a memory owned by `user_id` and return its id.
async fn insert_memory(state: &kleos_server::state::AppState, user_id: i64, content: &str) -> i64 {
    let content = content.to_string();
    state
        .db
        .write(move |conn| {
            Ok(conn.query_row(
                "INSERT INTO memories (user_id, content) VALUES (?1, ?2) RETURNING id",
                rusqlite::params![user_id, content],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .expect("insert memory")
}

/// Count surviving (non-deleted) memory rows for a user.
async fn memory_count(state: &kleos_server::state::AppState, user_id: i64) -> i64 {
    state
        .db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .expect("count memories")
}

#[tokio::test]
async fn admin_reset_wipes_only_the_caller_in_monolith_mode() {
    // test_app() runs in monolith mode (no tenant_registry).
    let (app, state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;

    // The admin caller is the owner (user 1). Seed a memory for them and one
    // for another tenant (user 2).
    let _owner_mem = insert_memory(&state, 1, "owner memory").await;
    let _other_mem = insert_memory(&state, 2, "other tenant memory").await;
    assert_eq!(memory_count(&state, 1).await, 1);
    assert_eq!(memory_count(&state, 2).await, 1);

    let (status, body) = post(
        &app,
        "/admin/reset",
        &admin_key,
        json!({ "confirm": "WIPE_ALL_MEMORIES" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reset failed: {body}");

    // The owner's data is gone; the other tenant's data is untouched.
    assert_eq!(
        memory_count(&state, 1).await,
        0,
        "caller's memories must be wiped"
    );
    assert_eq!(
        memory_count(&state, 2).await,
        1,
        "another tenant's memories must NOT be wiped"
    );
}
