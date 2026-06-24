//! Regression tests for store-path near-duplicate scoping (Phase 1.6).
//!
//! Near-duplicate detection must collapse genuine duplicates within one space, but must
//! NOT collapse the same content stored in a different space, nor short-circuit an
//! explicit version update (parent_memory_id set) into its predecessor.

use kleos_lib::db::Database;
use kleos_lib::memory;
use kleos_lib::memory::types::{StoreRequest, StoreResult};

/// Owner id for the tests.
const UID: i64 = 1;

/// Build a store request with explicit space and optional parent (version) link.
fn req(content: &str, space_id: Option<i64>, parent: Option<i64>) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: "general".to_string(),
        source: "test".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        chunk_embeddings: None,
        session_id: None,
        is_static: Some(false),
        user_id: Some(UID),
        space_id,
        parent_memory_id: parent,
        sync_id: None,
        artifacts: None,
    }
}

/// Store a memory and return the full result (id, created, duplicate_of).
async fn store(
    db: &Database,
    content: &str,
    space: Option<i64>,
    parent: Option<i64>,
) -> StoreResult {
    memory::store(db, req(content, space, parent), None, false)
        .await
        .expect("store")
}

/// Identical content in the same space collapses to one memory.
#[tokio::test]
async fn same_space_near_duplicate_collapses() {
    let db = Database::connect_memory().await.expect("db");
    let a = store(
        &db,
        "the quick brown fox jumps over the lazy dog",
        None,
        None,
    )
    .await;
    let b = store(
        &db,
        "the quick brown fox jumps over the lazy dog",
        None,
        None,
    )
    .await;
    assert!(a.created, "first store creates");
    assert!(!b.created, "identical same-space store must be deduped");
    assert_eq!(b.duplicate_of, Some(a.id));
}

/// Identical content in different spaces stays distinct.
#[tokio::test]
async fn different_space_is_not_deduped() {
    let db = Database::connect_memory().await.expect("db");
    let a = store(
        &db,
        "identical content stored across two spaces",
        Some(1),
        None,
    )
    .await;
    let b = store(
        &db,
        "identical content stored across two spaces",
        Some(2),
        None,
    )
    .await;
    assert!(a.created, "first store creates");
    assert!(
        b.created,
        "same content in a different space must not be deduped"
    );
    assert_ne!(a.id, b.id);
    assert_eq!(b.duplicate_of, None);
}

/// An explicit version update is never short-circuited as a duplicate of its parent.
#[tokio::test]
async fn version_update_is_not_deduped() {
    let db = Database::connect_memory().await.expect("db");
    let a = store(
        &db,
        "evolving design note in its initial revision",
        None,
        None,
    )
    .await;
    let b = store(
        &db,
        "evolving design note in its initial revision",
        None,
        Some(a.id),
    )
    .await;
    assert!(a.created, "first store creates");
    assert!(
        b.created,
        "a version update (parent set) must create a new row, not dedup into the parent"
    );
    assert_eq!(b.duplicate_of, None);
}
