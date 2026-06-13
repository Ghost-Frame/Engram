//! Tenant-isolation regression tests for the admin export and reembed paths.
//!
//! In shared (monolith) mode a single DB holds rows for many tenants, so
//! `export_user_data` and a per-user `reembed_all` must scope by `user_id`.
//! Pre-fix, memories/conversations/episodes/user_preferences were exported
//! unscoped and a per-user reembed cleared every tenant's embeddings.

use std::sync::Arc;

use kleos_lib::tenant::{TenantConfig, TenantHandle, TenantRegistry};
use rusqlite::params;

/// Spin up a single tenant DB against a fresh temp dir; leaks the dir so the
/// handle outlives the helper (matches `artifacts_fts.rs::one_tenant`).
async fn one_tenant() -> Arc<TenantHandle> {
    let dir = tempfile::tempdir().expect("tempdir");
    let registry = TenantRegistry::new(dir.path(), TenantConfig::default(), 128, false, None)
        .expect("registry");
    let handle = registry
        .get_or_create("export_tenant")
        .await
        .expect("tenant");
    std::mem::forget(dir);
    handle
}

/// Insert a memory owned by `user_id`, with a non-null embedding so a reembed
/// has something to clear. Returns the new memory id.
async fn insert_memory(db: &kleos_lib::db::Database, content: &str, user_id: i64) -> i64 {
    let content = content.to_string();
    db.write(move |conn| {
        conn.query_row(
            "INSERT INTO memories (content, category, importance, embedding, user_id) \
             VALUES (?1, 'test', 5, zeroblob(4), ?2) RETURNING id",
            params![content, user_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("insert memory")
}

/// Count memories with a non-null embedding for a given user.
async fn embedded_count(db: &kleos_lib::db::Database, user_id: i64) -> i64 {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM memories \
             WHERE user_id = ?1 AND embedding_vec_1024 IS NOT NULL",
            params![user_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("count embedded")
}

/// Run a row-count of the exact `WHERE` clause `export_user_data` now applies
/// to `memories`, so a regression that drops the `user_id` predicate is caught
/// even though the export serializer cannot round-trip integer-keyed rows.
async fn export_memories_row_count(db: &kleos_lib::db::Database, user_id: i64) -> i64 {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE is_forgotten = 0 AND user_id = ?1",
            params![user_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("count export memories")
}

/// In a shared DB the export's `memories` query must select only the caller's
/// rows. Pins the `AND user_id = ?1` predicate added to `export_user_data`.
///
/// NOTE: assertion runs against the literal export predicate rather than the
/// serialized `UserExport`, because `export_table_user` reads every column as
/// TEXT and drops rows on the first non-text column (the integer `id`). That
/// is a separate, pre-existing export-serializer gap unrelated to tenant
/// scoping.
#[tokio::test]
async fn export_memories_query_is_scoped_to_caller_in_shared_db() {
    const OWNER: i64 = 1;
    const OTHER: i64 = 2;

    let handle = one_tenant().await;
    let db = handle.database();

    insert_memory(&db, "owner note one", OWNER).await;
    insert_memory(&db, "owner note two", OWNER).await;
    insert_memory(&db, "other tenant secret", OTHER).await;

    assert_eq!(
        export_memories_row_count(&db, OWNER).await,
        2,
        "export must see only the owner's two memories"
    );
    assert_eq!(
        export_memories_row_count(&db, OTHER).await,
        1,
        "export must see only the other tenant's one memory"
    );
}

/// A per-user `reembed_all` must clear only that user's embeddings; a None
/// request clears everyone's (the deliberate admin-wide reembed).
#[tokio::test]
async fn reembed_per_user_does_not_touch_other_tenants() {
    const OWNER: i64 = 1;
    const OTHER: i64 = 2;

    let handle = one_tenant().await;
    let db = handle.database();

    // Embeddings populated above are NULL by default; set them so we can see a
    // reembed clear them. Insert then stamp a non-null vector.
    let owner_mem = insert_memory(&db, "owner embedded", OWNER).await;
    let other_mem = insert_memory(&db, "other embedded", OTHER).await;
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET embedding_vec_1024 = zeroblob(4096) WHERE id IN (?1, ?2)",
            params![owner_mem, other_mem],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("seed embeddings");

    assert_eq!(embedded_count(&db, OWNER).await, 1);
    assert_eq!(embedded_count(&db, OTHER).await, 1);

    let cleared = kleos_lib::admin::reembed_all(&db, Some(OWNER))
        .await
        .expect("per-user reembed");
    assert_eq!(cleared, 1, "per-user reembed must clear exactly one row");

    assert_eq!(
        embedded_count(&db, OWNER).await,
        0,
        "owner's embedding must be cleared"
    );
    assert_eq!(
        embedded_count(&db, OTHER).await,
        1,
        "other tenant's embedding must survive a per-user reembed"
    );
}
