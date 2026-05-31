//! Shared-DB isolation tests for intelligence decomposition and reconsolidation.
//!
//! These tests run against the monolith schema (`ENGRAM_TENANT_SHARDING=0`)
//! where row-level `user_id` predicates are the only tenant boundary.

use kleos_lib::db::Database;
use kleos_lib::intelligence::decomposition::decompose;
use kleos_lib::intelligence::reconsolidation::run_reconsolidation_sweep;
use kleos_lib::memory;
use kleos_lib::memory::types::StoreRequest;
use rusqlite::params;

/// Build a shared monolith database with the full migration chain applied.
async fn monolith() -> Database {
    Database::connect_memory().await.expect("monolith db")
}

/// Build a minimal store request owned by `user_id`.
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

/// Count child facts linked to `parent_id`.
async fn child_fact_count(db: &Database, parent_id: i64) -> i64 {
    db.read(move |conn| {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM memories WHERE parent_memory_id = ?1",
            params![parent_id],
            |row| row.get(0),
        )?)
    })
    .await
    .expect("count child facts")
}

/// List the owners of child facts linked to `parent_id`.
async fn child_fact_owners(db: &Database, parent_id: i64) -> Vec<i64> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare("SELECT user_id FROM memories WHERE parent_memory_id = ?1 ORDER BY id ASC")?;
        let mut rows = stmt.query(params![parent_id])?;
        let mut owners = Vec::new();
        while let Some(row) = rows.next()? {
            owners.push(row.get(0)?);
        }
        Ok(owners)
    })
    .await
    .expect("list child fact owners")
}

/// Read the `updated_at` timestamp for a memory row.
async fn memory_updated_at(db: &Database, memory_id: i64) -> String {
    db.read(move |conn| {
        Ok(conn.query_row(
            "SELECT updated_at FROM memories WHERE id = ?1",
            params![memory_id],
            |row| row.get(0),
        )?)
    })
    .await
    .expect("get memory updated_at")
}

/// Make a memory eligible for reconsolidation while forcing a deterministic
/// ordering within the candidate query.
async fn seed_recon_candidate(db: &Database, memory_id: i64, updated_at: &str) {
    let updated_at = updated_at.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories
             SET recall_hits = 4,
                 recall_misses = 0,
                 updated_at = ?2
             WHERE id = ?1",
            params![memory_id, updated_at],
        )?;
        Ok(())
    })
    .await
    .expect("seed reconsolidation candidate");
}

/// User B must not decompose a memory owned by user A in shared-DB mode.
#[tokio::test]
async fn intelligence_isolation_decompose_cross_tenant_returns_no_children() {
    let db = monolith().await;
    let parent_id = memory::store(
        &db,
        store_req(
            "Alice runs the API on port 8080 and Alice stores logs in /var/log/api.",
            10,
        ),
        None,
        false,
    )
    .await
    .expect("store parent")
    .id;

    let child_ids = decompose(&db, parent_id, 20).await.expect("decompose");

    assert!(
        child_ids.is_empty(),
        "cross-tenant decomposition must return no child ids"
    );
    assert_eq!(
        child_fact_count(&db, parent_id).await,
        0,
        "cross-tenant decomposition must not create child facts"
    );
}

/// Decomposition-created child facts must inherit the owner's `user_id`.
#[tokio::test]
async fn intelligence_isolation_decompose_child_facts_inherit_owner_user_id() {
    let db = monolith().await;
    let parent_id = memory::store(
        &db,
        store_req(
            "Alice runs the API on port 8080 and Alice stores logs in /var/log/api.",
            10,
        ),
        None,
        false,
    )
    .await
    .expect("store parent")
    .id;

    let child_ids = decompose(&db, parent_id, 10).await.expect("decompose");

    assert!(
        !child_ids.is_empty(),
        "owner decomposition must create child facts"
    );
    assert!(
        child_fact_owners(&db, parent_id)
            .await
            .into_iter()
            .all(|user_id| user_id == 10),
        "every child fact must inherit the owner's user_id"
    );
}

/// Reconsolidation sweeps must only consider candidate memories owned by the
/// caller in shared-DB mode.
#[tokio::test]
async fn intelligence_isolation_reconsolidation_cross_tenant_candidate_selection_is_scoped() {
    let db = monolith().await;
    let alice_id = memory::store(&db, store_req("Alice recall target", 10), None, false)
        .await
        .expect("store alice")
        .id;
    let bob_id = memory::store(&db, store_req("Bob recall target", 20), None, false)
        .await
        .expect("store bob")
        .id;

    seed_recon_candidate(&db, alice_id, "2024-01-02 00:00:00").await;
    seed_recon_candidate(&db, bob_id, "2024-01-01 00:00:00").await;
    let bob_updated_before = memory_updated_at(&db, bob_id).await;

    let results = run_reconsolidation_sweep(&db, 10, 1)
        .await
        .expect("run reconsolidation sweep");

    assert_eq!(
        results.len(),
        1,
        "shared-DB sweep must return one in-scope candidate"
    );
    assert_eq!(
        results[0].memory_id, alice_id,
        "shared-DB sweep must skip older cross-tenant candidates"
    );
    assert_eq!(
        memory_updated_at(&db, bob_id).await,
        bob_updated_before,
        "cross-tenant candidate rows must remain untouched"
    );
}
