//! Integration tests for memory version-chain integrity (round1 C6 / V4).
//!
//! These guard two regressions in the version-chain write paths:
//!   - storing a child against an already superseded parent must NOT fork the
//!     chain into two live heads (`is_latest = 1`);
//!   - updating a memory's content must carry forward the previous version's
//!     lifecycle/linkage fields rather than resetting them to table defaults.
//!
//! Each test runs against a fresh isolated tenant database.

use std::sync::Arc;
use tempfile::tempdir;

use kleos_lib::memory::types::{StoreRequest, UpdateRequest};
use kleos_lib::memory::{self};
use kleos_lib::tenant::{TenantConfig, TenantHandle, TenantRegistry};

/// Spin up a single isolated tenant handle backed by a temporary directory.
async fn single_tenant() -> Arc<TenantHandle> {
    let dir = tempdir().expect("tempdir");
    let registry = TenantRegistry::new(dir.path(), TenantConfig::default(), 128, false, None)
        .expect("registry");
    let handle = registry.get_or_create("test_tenant").await.expect("tenant");
    // Keep the backing files alive for the test duration.
    std::mem::forget(dir);
    handle
}

/// Build a minimal store request for `content`.
fn base_store(content: &str) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: "general".to_string(),
        source: "test".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        session_id: None,
        is_static: None,
        user_id: Some(1),
        space_id: None,
        parent_memory_id: None,
        chunk_embeddings: None,
        sync_id: None,
        artifacts: None,
    }
}

/// Build an update request that only changes the content.
fn content_update(new_content: &str) -> UpdateRequest {
    UpdateRequest {
        content: Some(new_content.to_string()),
        category: None,
        importance: None,
        tags: None,
        is_static: None,
        status: None,
        embedding: None,
        chunk_embeddings: None,
    }
}

/// Updating content must preserve lifecycle/linkage fields on the new version.
#[tokio::test]
async fn update_preserves_lifecycle_fields() {
    let tenant = single_tenant().await;
    let db = tenant.database();

    let stored = memory::store(&db, base_store("original content"), None, false)
        .await
        .expect("store");
    let id = stored.id;

    // Set lifecycle/linkage fields that are not exposed through StoreRequest.
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET is_archived = 1, is_fact = 1, valence = 0.5, \
                 dominant_emotion = 'joy', source_count = 3, forget_after = '2099-01-01' \
             WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("seed lifecycle fields");

    let new = memory::update(&db, id, content_update("updated content"), 1, false)
        .await
        .expect("update");
    let new_id = new.id;

    #[allow(clippy::type_complexity)]
    let (is_archived, is_fact, valence, emotion, source_count, forget_after): (
        i64,
        i64,
        Option<f64>,
        Option<String>,
        i64,
        Option<String>,
    ) = db
        .read(move |conn| {
            conn.query_row(
                "SELECT is_archived, is_fact, valence, dominant_emotion, source_count, forget_after \
                 FROM memories WHERE id = ?1",
                rusqlite::params![new_id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("read new version row");

    assert_eq!(is_archived, 1, "is_archived must carry forward");
    assert_eq!(is_fact, 1, "is_fact must carry forward");
    assert_eq!(valence, Some(0.5), "valence must carry forward");
    assert_eq!(
        emotion.as_deref(),
        Some("joy"),
        "emotion must carry forward"
    );
    assert_eq!(source_count, 3, "source_count must carry forward");
    assert_eq!(
        forget_after.as_deref(),
        Some("2099-01-01"),
        "forget_after must carry forward"
    );
}

/// Storing a child against a superseded parent must be refused, not forked.
#[tokio::test]
async fn store_with_superseded_parent_does_not_fork() {
    let tenant = single_tenant().await;
    let db = tenant.database();

    let a = memory::store(&db, base_store("A v1"), None, false)
        .await
        .expect("store A");
    let a_id = a.id;

    // Update A -> A' so A is no longer the latest version.
    let a_prime = memory::update(&db, a_id, content_update("A v2"), 1, false)
        .await
        .expect("update A");
    let root_id = a_prime.root_memory_id.unwrap_or(a_id);

    // Storing a child whose parent is the now-stale original A must error.
    let mut child = base_store("child of stale parent");
    child.parent_memory_id = Some(a_id);
    let res = memory::store(&db, child, None, false).await;
    assert!(
        res.is_err(),
        "storing against a superseded parent must be refused, not forked"
    );

    // The chain must have exactly one live head.
    let live_heads: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memories \
                 WHERE (id = ?1 OR root_memory_id = ?1) AND is_latest = 1",
                rusqlite::params![root_id],
                |r| r.get(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("count live heads");
    assert_eq!(live_heads, 1, "chain must have exactly one live head");
}
