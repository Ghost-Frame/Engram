//! Shared-DB mutation IDOR regression tests.
//!
//! These tests run two users against one monolith database so row-level
//! `user_id` predicates are the only authorization boundary.

use kleos_lib::db::Database;
use kleos_lib::memory;
use kleos_lib::memory::types::{StoreRequest, UpdateRequest};

/// Build a shared monolith in-memory database.
async fn monolith() -> Database {
    Database::connect_memory().await.expect("monolith db")
}

/// Build a minimal memory store request owned by `user_id`.
fn store_req(content: &str, user_id: i64) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: "general".to_string(),
        source: "test".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        chunk_embeddings: None,
        session_id: None,
        is_static: None,
        user_id: Some(user_id),
        space_id: None,
        parent_memory_id: None,
        sync_id: None,
        artifacts: None,
    }
}

/// Store a memory for `user_id` and return its row id.
async fn store_memory(db: &Database, content: &str, user_id: i64) -> i64 {
    memory::store(db, store_req(content, user_id), None, false)
        .await
        .expect("store memory")
        .id
}

/// Read the current memory flags used by mutation tests.
async fn memory_flags(db: &Database, memory_id: i64) -> (bool, bool, i32, Option<String>) {
    db.read(move |conn| {
        Ok(conn.query_row(
            "SELECT is_forgotten, is_archived, importance, forget_reason
             FROM memories
             WHERE id = ?1",
            rusqlite::params![memory_id],
            |row| {
                Ok((
                    row.get::<_, i32>(0)? != 0,
                    row.get::<_, i32>(1)? != 0,
                    row.get(2)?,
                    row.get(3)?,
                ))
            },
        )?)
    })
    .await
    .expect("read memory flags")
}

/// Read the singleton pagerank dirty counter.
async fn pagerank_dirty_count(db: &Database) -> i64 {
    db.read(move |conn| {
        Ok(conn.query_row(
            "SELECT dirty_count FROM pagerank_dirty WHERE id = 1",
            [],
            |row| row.get(0),
        )?)
    })
    .await
    .expect("read pagerank dirty count")
}

/// User A must not mutate lifecycle fields on user B's memory by id.
#[tokio::test]
async fn mutation_idor_blocks_cross_user_lifecycle_updates() {
    let db = monolith().await;
    let bob_id = store_memory(&db, "bob owned memory", 20).await;

    assert!(memory::mark_forgotten(&db, bob_id, 10).await.is_err());
    assert!(memory::mark_archived(&db, bob_id, 10).await.is_err());
    assert!(memory::mark_unarchived(&db, bob_id, 10).await.is_err());
    assert!(memory::update_forget_reason(&db, bob_id, "not yours", 10)
        .await
        .is_err());
    assert!(memory::adjust_importance(&db, bob_id, 10, 3).await.is_err());

    let (forgotten, archived, importance, reason) = memory_flags(&db, bob_id).await;
    assert!(!forgotten, "cross-user forget must not update the row");
    assert!(!archived, "cross-user archive must not update the row");
    assert_eq!(
        importance, 5,
        "cross-user importance must not update the row"
    );
    assert!(
        reason.is_none(),
        "cross-user reason must not update the row"
    );
}

/// Owners must still be able to mutate their own memory rows.
#[tokio::test]
async fn mutation_idor_allows_owner_lifecycle_updates() {
    let db = monolith().await;
    let alice_id = store_memory(&db, "alice owned memory", 10).await;

    memory::mark_archived(&db, alice_id, 10)
        .await
        .expect("owner archive");
    memory::update_forget_reason(&db, alice_id, "owner reason", 10)
        .await
        .expect("owner reason");
    memory::adjust_importance(&db, alice_id, 10, 2)
        .await
        .expect("owner importance");

    let (_forgotten, archived, importance, reason) = memory_flags(&db, alice_id).await;
    assert!(archived, "owner archive should update the row");
    assert_eq!(importance, 7, "owner importance should increase");
    assert_eq!(reason.as_deref(), Some("owner reason"));
}

/// User A must not create a graph link involving user B's memory.
#[tokio::test]
async fn mutation_idor_blocks_cross_user_links() {
    let db = monolith().await;
    let alice_id = store_memory(&db, "alice memory", 10).await;
    let bob_id = store_memory(&db, "bob memory", 20).await;

    assert!(
        memory::insert_link(&db, alice_id, bob_id, 1.0, "manual", 10)
            .await
            .is_err(),
        "alice must not link to bob memory"
    );

    let link_count: i64 = db
        .read(move |conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM memory_links", [], |row| row.get(0))?)
        })
        .await
        .expect("count links");
    assert_eq!(link_count, 0, "cross-user link must not be inserted");
}

/// Version-chain lookup must remain scoped to the authenticated owner.
#[tokio::test]
async fn mutation_idor_scopes_version_chain_to_owner() {
    let db = monolith().await;
    let alice_id = store_memory(&db, "alice v1", 10).await;
    let bob_id = store_memory(&db, "bob v1", 20).await;

    let _alice_v2 = memory::update(
        &db,
        alice_id,
        UpdateRequest {
            content: Some("alice v2".to_string()),
            category: None,
            importance: None,
            tags: None,
            is_static: None,
            status: None,
            embedding: None,
            chunk_embeddings: None,
        },
        10,
        false,
    )
    .await
    .expect("alice update");
    let _bob_v2 = memory::update(
        &db,
        bob_id,
        UpdateRequest {
            content: Some("bob v2".to_string()),
            category: None,
            importance: None,
            tags: None,
            is_static: None,
            status: None,
            embedding: None,
            chunk_embeddings: None,
        },
        20,
        false,
    )
    .await
    .expect("bob update");

    let alice_chain = memory::get_version_chain(&db, alice_id, 10)
        .await
        .expect("alice chain");
    assert_eq!(alice_chain.len(), 2);
    assert!(
        alice_chain
            .iter()
            .all(|entry| entry.content.starts_with("alice")),
        "alice version chain must not include bob rows"
    );
    assert!(
        memory::get_version_chain(&db, bob_id, 10).await.is_err(),
        "alice must not read bob chain by id"
    );
}

/// Memory mutations mark the singleton pagerank dirty counter by one.
#[tokio::test]
async fn mutation_idor_marks_pagerank_dirty_once() {
    let db = monolith().await;
    let alice_id = store_memory(&db, "alice dirty marker", 10).await;
    let before = pagerank_dirty_count(&db).await;

    memory::mark_archived(&db, alice_id, 10)
        .await
        .expect("owner archive");

    let after = pagerank_dirty_count(&db).await;
    assert_eq!(
        after - before,
        1,
        "mark_pagerank_dirty takes a delta, not a user id"
    );
}
