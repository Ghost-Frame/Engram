//! Tenant-isolation regression tests for the inbox (pending-memory) path.
//!
//! In shared (monolith) mode a single DB holds many tenants' rows, so every
//! inbox operation must scope by `user_id`. Pre-fix, list/count returned the
//! whole pending queue and approve/reject/edit mutated any row by id, letting
//! one tenant act on another's pending memories.

use std::sync::Arc;

use kleos_lib::tenant::{TenantConfig, TenantHandle, TenantRegistry};
use rusqlite::params;

/// Spin up a single tenant DB against a fresh temp dir; leaks the dir so the
/// handle outlives the helper.
async fn one_tenant() -> Arc<TenantHandle> {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = TenantRegistry::new(dir.path(), TenantConfig::default(), 128, false, None)
        .expect("registry");
    let handle = registry
        .get_or_create("inbox_tenant")
        .await
        .expect("tenant");
    std::mem::forget(dir);
    handle
}

/// Insert a pending memory owned by `user_id`; returns its id.
async fn insert_pending(db: &kleos_lib::db::Database, content: &str, user_id: i64) -> i64 {
    let content = content.to_string();
    db.write(move |conn| {
        conn.query_row(
            "INSERT INTO memories (content, category, importance, status, user_id) \
             VALUES (?1, 'test', 5, 'pending', ?2) RETURNING id",
            params![content, user_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("insert pending memory")
}

/// Read a memory's status by id (no user scoping, so the test can observe
/// another tenant's row directly).
async fn status_of(db: &kleos_lib::db::Database, id: i64) -> String {
    db.read(move |conn| {
        conn.query_row(
            "SELECT status FROM memories WHERE id = ?1",
            params![id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("read status")
}

/// `list_pending` / `count_pending` must return only the caller's pending rows.
#[tokio::test]
async fn list_pending_is_scoped_to_caller() {
    const OWNER: i64 = 1;
    const OTHER: i64 = 2;

    let handle = one_tenant().await;
    let db = handle.database();

    insert_pending(&db, "owner pending one", OWNER).await;
    insert_pending(&db, "owner pending two", OWNER).await;
    insert_pending(&db, "other pending", OTHER).await;

    let owner_list = kleos_lib::inbox::list_pending(&db, OWNER, 50, 0)
        .await
        .expect("owner list");
    assert_eq!(owner_list.len(), 2, "owner sees only its two pending rows");
    assert_eq!(
        kleos_lib::inbox::count_pending(&db, OWNER).await.unwrap(),
        2
    );

    let other_list = kleos_lib::inbox::list_pending(&db, OTHER, 50, 0)
        .await
        .expect("other list");
    assert_eq!(other_list.len(), 1, "other tenant sees only its one row");
    assert_eq!(
        kleos_lib::inbox::count_pending(&db, OTHER).await.unwrap(),
        1
    );
}

/// approve/reject/edit must not mutate another tenant's pending memory by id.
#[tokio::test]
async fn inbox_mutations_are_scoped_to_caller() {
    const OWNER: i64 = 1;
    const INTRUDER: i64 = 2;

    let handle = one_tenant().await;
    let db = handle.database();

    let m_approve = insert_pending(&db, "to approve", OWNER).await;
    let m_reject = insert_pending(&db, "to reject", OWNER).await;
    let m_edit = insert_pending(&db, "to edit", OWNER).await;

    // Intruder attempts on every id are no-ops: rows stay pending.
    kleos_lib::inbox::approve_memory(&db, m_approve, INTRUDER)
        .await
        .expect("intruder approve");
    kleos_lib::inbox::reject_memory(&db, m_reject, INTRUDER)
        .await
        .expect("intruder reject");
    kleos_lib::inbox::edit_and_approve(&db, m_edit, INTRUDER, Some("hijacked"), None, None, None)
        .await
        .expect("intruder edit");

    assert_eq!(status_of(&db, m_approve).await, "pending");
    assert_eq!(status_of(&db, m_reject).await, "pending");
    assert_eq!(status_of(&db, m_edit).await, "pending");

    // The owner's own operations take effect.
    kleos_lib::inbox::approve_memory(&db, m_approve, OWNER)
        .await
        .expect("owner approve");
    kleos_lib::inbox::reject_memory(&db, m_reject, OWNER)
        .await
        .expect("owner reject");
    kleos_lib::inbox::edit_and_approve(&db, m_edit, OWNER, Some("owned edit"), None, None, None)
        .await
        .expect("owner edit");

    assert_eq!(status_of(&db, m_approve).await, "approved");
    assert_eq!(status_of(&db, m_reject).await, "rejected");
    assert_eq!(status_of(&db, m_edit).await, "approved");

    // The intruder's edit content never landed.
    let edited_content = db
        .read(move |conn| {
            conn.query_row(
                "SELECT content FROM memories WHERE id = ?1",
                params![m_edit],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("read content");
    assert_eq!(
        edited_content, "owned edit",
        "intruder edit must not persist"
    );
}
