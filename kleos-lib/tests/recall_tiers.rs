//! Regression tests for the recall "static" and "important" tiers (Phase 1.1 / 1.2).
//!
//! The recall endpoint must always surface pinned/static memories and high-importance
//! memories regardless of how recently they were written. The previous implementation
//! listed the newest N rows and filtered afterwards, so any pinned or important memory
//! older than that window silently disappeared. These tests seed old pinned/important
//! rows behind a wall of newer, low-importance rows and assert they still surface via
//! `memory::list_static` / `memory::list_important`.

use kleos_lib::db::Database;
use kleos_lib::memory;
use kleos_lib::memory::types::StoreRequest;

/// Owner id for the tests.
const UID: i64 = 1;

/// Build a store request with explicit static flag, importance, and space.
fn req(content: &str, importance: i32, is_static: bool, space_id: Option<i64>) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: "general".to_string(),
        source: "test".to_string(),
        importance,
        tags: None,
        embedding: None,
        chunk_embeddings: None,
        session_id: None,
        is_static: Some(is_static),
        user_id: Some(UID),
        space_id,
        parent_memory_id: None,
        sync_id: None,
        artifacts: None,
    }
}

/// Store one memory and return its row id.
async fn store(
    db: &Database,
    content: &str,
    importance: i32,
    is_static: bool,
    space: Option<i64>,
) -> i64 {
    memory::store(db, req(content, importance, is_static, space), None, false)
        .await
        .expect("store")
        .id
}

/// Seed an old pinned and an old high-importance memory, then bury them under many newer
/// low-importance rows. Both must still surface through the dedicated tier queries.
#[tokio::test]
async fn static_and_important_survive_the_recency_window() {
    let db = Database::connect_memory().await.expect("db");

    // Oldest rows: one pinned, one high-importance. They will fall outside any
    // newest-10 or newest-20 window once the filler below is stored.
    let old_static = store(
        &db,
        "permanent pinned identity fact for the owner",
        8,
        true,
        None,
    )
    .await;
    let old_important = store(
        &db,
        "critical high importance architecture decision",
        10,
        false,
        None,
    )
    .await;

    // 25 newer, distinct, low-importance rows so the two old rows are well outside the
    // old recency windows (10 and 20).
    for i in 0..25 {
        let content =
            format!("routine log entry {i} concerning widget {i} and gadget {i} status report");
        store(&db, &content, 4, false, None).await;
    }

    // Static tier: the pinned fact must be present despite being the oldest row.
    let statics = memory::list_static(&db, UID, None, RECALL_STATIC_LIMIT)
        .await
        .expect("list_static");
    let static_ids: Vec<i64> = statics.iter().map(|m| m.id).collect();
    assert!(
        static_ids.contains(&old_static),
        "old pinned/static memory must surface in the static tier; got {static_ids:?}"
    );
    assert!(
        statics.iter().all(|m| m.is_static),
        "list_static must only return static memories"
    );

    // Important tier: the high-importance decision must be present and ranked first
    // (importance 10 ahead of the pinned importance-8 row), and the importance-4 filler
    // must be excluded.
    let important = memory::list_important(&db, UID, None, 7, 10)
        .await
        .expect("list_important");
    let important_ids: Vec<i64> = important.iter().map(|m| m.id).collect();
    assert!(
        important_ids.contains(&old_important),
        "old high-importance memory must surface in the important tier; got {important_ids:?}"
    );
    assert_eq!(
        important.first().map(|m| m.id),
        Some(old_important),
        "important tier must be ordered by importance (10 before 8)"
    );
    assert!(
        important.iter().all(|m| m.importance >= 7),
        "list_important must respect the importance floor"
    );
}

/// Static tier must respect space scoping.
#[tokio::test]
async fn static_tier_respects_space_scope() {
    let db = Database::connect_memory().await.expect("db");

    let in_space = store(&db, "pinned fact scoped to space five", 6, true, Some(5)).await;
    let other_space = store(&db, "pinned fact scoped to space nine", 6, true, Some(9)).await;
    let no_space = store(&db, "pinned fact with no space", 6, true, None).await;

    let scoped = memory::list_static(&db, UID, Some(5), 25)
        .await
        .expect("list_static scoped");
    let ids: Vec<i64> = scoped.iter().map(|m| m.id).collect();
    assert!(ids.contains(&in_space), "space-5 pinned fact must surface");
    assert!(
        !ids.contains(&other_space),
        "space-9 pinned fact must not leak into space-5"
    );
    assert!(
        !ids.contains(&no_space),
        "unscoped pinned fact must not match a space filter"
    );
}

/// Cap for the static tier mirrored from the server route default.
const RECALL_STATIC_LIMIT: usize = 25;
