//! Regression tests for the artifact FTS indexing path.
//!
//! Background: `artifacts_fts` is an external-content FTS5 virtual table
//! (`content='artifacts' content_rowid='id'`) maintained by AFTER
//! INSERT/UPDATE/DELETE triggers on the `artifacts` table. The application
//! must therefore NOT issue its own INSERTs against `artifacts_fts`; doing so
//! produces duplicate rows for the same `rowid`. These tests pin that
//! invariant so the bug fixed in `feat/artifact-completion` C1 cannot regress.

use std::sync::Arc;

use kleos_lib::artifacts::{store_artifact, StoreArtifactOpts};
use kleos_lib::tenant::{TenantConfig, TenantHandle, TenantRegistry};
use rusqlite::params;
use tempfile::tempdir;

/// Spin up a single tenant against a fresh temp dir; leaks the dir so the
/// handle outlives the helper (matches `tenant_isolation.rs::two_tenants`).
async fn one_tenant() -> Arc<TenantHandle> {
    let dir = tempdir().expect("tempdir");
    let registry = TenantRegistry::new(dir.path(), TenantConfig::default(), 128, false, None)
        .expect("registry");
    let handle = registry.get_or_create("fts_tenant").await.expect("tenant");
    std::mem::forget(dir);
    handle
}

/// Insert a minimal memory row directly via SQL so artifacts have a parent to
/// reference (memory_id FK with ON DELETE CASCADE).
async fn insert_memory(db: &kleos_lib::db::Database, content: &str) -> i64 {
    let content = content.to_string();
    db.write(move |conn| {
        conn.query_row(
            "INSERT INTO memories (content, category, importance, embedding) \
             VALUES (?1, 'test', 5, zeroblob(4)) RETURNING id",
            params![content],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("insert memory")
}

/// Count rows in `artifacts_fts` for a given rowid. With the C1 fix in place
/// this must be exactly 1 for an indexable artifact; without the fix the old
/// `index_artifact()` call produced 2.
async fn fts_rowid_count(db: &kleos_lib::db::Database, rowid: i64) -> i64 {
    db.read(move |conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM artifacts_fts WHERE rowid = ?1",
            params![rowid],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("count fts rows")
}

/// Regression for the C1 bug: storing an indexable artifact must produce
/// exactly one row in `artifacts_fts`. Pre-fix, the AFTER INSERT trigger plus
/// the now-deleted `index_artifact()` call yielded two rows per upload.
#[tokio::test]
async fn indexable_artifact_creates_single_fts_row() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for fts test").await;

    let content = "server { listen 80; upstream backend { server 127.0.0.1; } }";
    let data = content.as_bytes().to_vec();
    let opts = StoreArtifactOpts {
        artifact_type: Some("file".into()),
        content: Some(content.to_string()),
        source_url: None,
        agent: None,
        session_id: None,
        metadata: None,
    };

    let artifact_id = store_artifact(
        &db,
        memory_id,
        "nginx.conf",
        "nginx.conf",
        "text/plain",
        data.len() as i64,
        "deadbeef",
        "inline",
        Some(data),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact");

    let count = fts_rowid_count(&db, artifact_id).await;
    assert_eq!(
        count, 1,
        "exactly one artifacts_fts row per artifact (got {count})"
    );
}

/// End-to-end FTS path: an indexable artifact's content must be reachable
/// via `MATCH` against `artifacts_fts`. Catches both trigger regressions and
/// migration ordering bugs that leave `artifacts_fts` absent in a tenant DB.
#[tokio::test]
async fn indexable_artifact_is_searchable_by_content() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for fts search").await;
    let content = "configuration directive: upstream backend pool";
    let opts = StoreArtifactOpts {
        artifact_type: Some("file".into()),
        content: Some(content.to_string()),
        ..StoreArtifactOpts::default()
    };

    let artifact_id = store_artifact(
        &db,
        memory_id,
        "config.txt",
        "config.txt",
        "text/plain",
        content.len() as i64,
        "cafef00d",
        "inline",
        Some(content.as_bytes().to_vec()),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact");

    let hit_rowid: Option<i64> = db
        .read(move |conn| {
            conn.query_row(
                "SELECT rowid FROM artifacts_fts WHERE artifacts_fts MATCH 'upstream'",
                params![],
                |row| row.get::<_, i64>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(kleos_lib::EngError::DatabaseMessage(other.to_string())),
            })
        })
        .await
        .expect("fts query");

    assert_eq!(hit_rowid, Some(artifact_id));
}

/// Non-indexable MIME types (e.g. image/png) must NOT set `is_indexed=1`.
/// Pins the inline-default behavior of `store_artifact` after the
/// post-insert UPDATE in `index_artifact()` was removed.
#[tokio::test]
async fn binary_artifact_has_no_indexable_content() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for binary artifact").await;
    let png_header = vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

    let artifact_id = store_artifact(
        &db,
        memory_id,
        "logo.png",
        "logo.png",
        "image/png",
        png_header.len() as i64,
        "feedface",
        "inline",
        Some(png_header),
        None,
        false,
        &StoreArtifactOpts::default(),
    )
    .await
    .expect("store artifact");

    let is_indexed: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT is_indexed FROM artifacts WHERE id = ?1",
                params![artifact_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("read is_indexed");

    assert_eq!(is_indexed, 0, "binary artifact must not set is_indexed=1");
}

/// Deleting an indexable artifact must remove its FTS row (via the
/// `artifacts_fts_delete` trigger) and make the artifact unreachable via
/// `get_artifact_by_id`.
#[tokio::test]
async fn delete_artifact_removes_fts_row() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for delete test").await;
    let content = "removable configuration directive";
    let data = content.as_bytes().to_vec();
    let opts = StoreArtifactOpts {
        artifact_type: Some("file".into()),
        content: Some(content.to_string()),
        ..StoreArtifactOpts::default()
    };

    let artifact_id = store_artifact(
        &db,
        memory_id,
        "delete-me.conf",
        "delete-me.conf",
        "text/plain",
        data.len() as i64,
        "aabbccdd",
        "inline",
        Some(data),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact");

    // Sanity: FTS row exists before deletion.
    assert_eq!(fts_rowid_count(&db, artifact_id).await, 1);

    let disk_path = kleos_lib::artifacts::delete_artifact(&db, artifact_id)
        .await
        .expect("delete artifact");
    assert!(disk_path.is_none(), "inline artifact has no disk path");

    // FTS row must be gone.
    assert_eq!(
        fts_rowid_count(&db, artifact_id).await,
        0,
        "FTS row must be removed after delete"
    );

    // Artifact must be unreachable.
    let gone = kleos_lib::artifacts::get_artifact_by_id(&db, artifact_id)
        .await
        .expect("get after delete");
    assert!(gone.is_none(), "artifact must be gone after delete");
}

/// Deleting a nonexistent artifact is idempotent -- returns Ok(None).
#[tokio::test]
async fn delete_nonexistent_artifact_returns_none() {
    let handle = one_tenant().await;
    let db = handle.database();

    let result = kleos_lib::artifacts::delete_artifact(&db, 999999)
        .await
        .expect("delete nonexistent");
    assert!(
        result.is_none(),
        "deleting nonexistent artifact should return None"
    );
}

/// `search_artifacts` must find an artifact by a token present in its
/// indexed content and return the correct artifact ID.
#[tokio::test]
async fn search_artifacts_finds_by_content() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for search test").await;
    let content = "quantum entanglement protocol specification";
    let data = content.as_bytes().to_vec();
    let opts = StoreArtifactOpts {
        artifact_type: Some("file".into()),
        content: Some(content.to_string()),
        ..StoreArtifactOpts::default()
    };

    let artifact_id = store_artifact(
        &db,
        memory_id,
        "quantum.txt",
        "quantum.txt",
        "text/plain",
        data.len() as i64,
        "1111aaaa",
        "inline",
        Some(data),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact");

    let results = kleos_lib::artifacts::search_artifacts(&db, "entanglement", 10, None)
        .await
        .expect("search artifacts");

    assert_eq!(results.len(), 1, "expected exactly one search hit");
    assert_eq!(results[0].id, artifact_id);
}

/// Searching for a term that matches no indexed content returns an empty vec.
#[tokio::test]
async fn search_artifacts_empty_result() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for empty search test").await;
    let content = "ordinary configuration data";
    let data = content.as_bytes().to_vec();
    let opts = StoreArtifactOpts {
        artifact_type: Some("file".into()),
        content: Some(content.to_string()),
        ..StoreArtifactOpts::default()
    };

    store_artifact(
        &db,
        memory_id,
        "normal.txt",
        "normal.txt",
        "text/plain",
        data.len() as i64,
        "2222bbbb",
        "inline",
        Some(data),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact");

    let results = kleos_lib::artifacts::search_artifacts(&db, "xylophone", 10, None)
        .await
        .expect("search artifacts");

    assert!(results.is_empty(), "nonexistent term should yield no hits");
}

/// When `memory_id` is provided, `search_artifacts` must only return
/// artifacts attached to that specific memory.
#[tokio::test]
async fn search_artifacts_respects_memory_filter() {
    let handle = one_tenant().await;
    let db = handle.database();

    let mem_a = insert_memory(&db, "memory alpha").await;
    let mem_b = insert_memory(&db, "memory beta").await;

    let content = "shared keyword: synchronization protocol";
    let data = content.as_bytes().to_vec();
    let opts = StoreArtifactOpts {
        artifact_type: Some("file".into()),
        content: Some(content.to_string()),
        ..StoreArtifactOpts::default()
    };

    let id_a = store_artifact(
        &db,
        mem_a,
        "sync-a.txt",
        "sync-a.txt",
        "text/plain",
        data.len() as i64,
        "3333cccc",
        "inline",
        Some(data.clone()),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact on mem_a");

    let _id_b = store_artifact(
        &db,
        mem_b,
        "sync-b.txt",
        "sync-b.txt",
        "text/plain",
        data.len() as i64,
        "4444dddd",
        "inline",
        Some(data),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact on mem_b");

    // Unfiltered search should find both.
    let all = kleos_lib::artifacts::search_artifacts(&db, "synchronization", 10, None)
        .await
        .expect("unfiltered search");
    assert_eq!(all.len(), 2, "unfiltered search should find both artifacts");

    // Filtered search should find only mem_a's artifact.
    let filtered = kleos_lib::artifacts::search_artifacts(&db, "synchronization", 10, Some(mem_a))
        .await
        .expect("filtered search");
    assert_eq!(
        filtered.len(),
        1,
        "filtered search should find one artifact"
    );
    assert_eq!(filtered[0].id, id_a);
}

// ---------------------------------------------------------------------------
// Storage quota enforcement (C4)
// ---------------------------------------------------------------------------

/// `enforce_storage_quota` must allow an upload when total usage plus the
/// new upload stays under the default 1 GiB limit.
#[tokio::test]
async fn storage_quota_allows_within_limit() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for quota allow test").await;
    let data = vec![0u8; 5000];
    let opts = StoreArtifactOpts::default();

    store_artifact(
        &db,
        memory_id,
        "small.bin",
        "small.bin",
        "application/octet-stream",
        data.len() as i64,
        "aaaa1111",
        "inline",
        Some(data),
        None,
        false,
        &opts,
    )
    .await
    .expect("store artifact");

    // Adding another 5000 bytes should pass (well under 1 GiB).
    let result = kleos_lib::quota::enforce_storage_quota(&db, 5000).await;
    assert!(result.is_ok(), "upload within limit should succeed");
}

/// `enforce_storage_quota` must reject an upload when total usage plus the
/// new upload would exceed the default 1 GiB limit.
#[tokio::test]
async fn storage_quota_rejects_over_limit() {
    let handle = one_tenant().await;
    let db = handle.database();

    let memory_id = insert_memory(&db, "host for quota reject test").await;

    // Fake a large existing artifact by inserting directly (we can't allocate
    // 1 GiB in a test, so we insert a row with a large size_bytes value).
    let limit = kleos_lib::quota::DEFAULT_STORAGE_BYTES_LIMIT;
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO artifacts (name, memory_id, filename, artifact_type, mime_type, \
             size_bytes, sha256, storage_mode, is_encrypted, is_indexed) \
             VALUES ('big.bin', ?1, 'big.bin', 'file', 'application/octet-stream', \
             ?2, 'ffff0000', 'inline', 0, 0)",
            rusqlite::params![memory_id, limit - 100],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
    })
    .await
    .expect("insert large artifact row");

    // Attempting to upload 200 more bytes should exceed the limit.
    let result = kleos_lib::quota::enforce_storage_quota(&db, 200).await;
    assert!(result.is_err(), "upload exceeding limit should fail");

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("storage quota exceeded"),
        "error should mention storage quota: {msg}"
    );
}

/// The default storage limit is 1 GiB (1073741824 bytes).
#[test]
fn storage_quota_default_is_1gib() {
    assert_eq!(kleos_lib::quota::DEFAULT_STORAGE_BYTES_LIMIT, 1_073_741_824);
}
