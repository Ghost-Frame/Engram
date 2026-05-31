//! Shared-DB scoping regressions for Batch H intelligence functions.
//!
//! These run two users against one monolith database and assert that the
//! temporal, predictive, duplicate, consolidation-candidate, and growth read
//! paths never observe another user's rows when `ENGRAM_TENANT_SHARDING=0`.
//! They mirror the Batch A1 read-isolation suite for the functions the
//! original remediation left unscoped.

use kleos_lib::db::Database;
use kleos_lib::intelligence::consolidation::find_consolidation_candidates;
use kleos_lib::intelligence::duplicates::find_duplicates;
use kleos_lib::intelligence::growth::{list_observations, materialize};
use kleos_lib::intelligence::predictive::detect_sequence_patterns;
use kleos_lib::intelligence::temporal::{detect_patterns, time_travel};
use kleos_lib::memory;
use kleos_lib::memory::types::StoreRequest;

/// Build a shared monolith in-memory database.
async fn monolith() -> Database {
    Database::connect_memory().await.expect("monolith db")
}

/// Build a minimal store request owned by `user_id` in a given category.
fn store_req(content: &str, category: &str, user_id: i64) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: category.to_string(),
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

/// Store a memory and return its id.
async fn store(db: &Database, content: &str, category: &str, user_id: i64) -> i64 {
    memory::store(db, store_req(content, category, user_id), None, false)
        .await
        .expect("store memory")
        .id
}

/// Attempt to insert a raw memory_links row, returning the result so callers
/// can assert success (same-owner) or rejection by the
/// `prevent_cross_tenant_links` trigger (cross-owner).
async fn raw_link(
    db: &Database,
    source_id: i64,
    target_id: i64,
    link_type: &str,
) -> kleos_lib::Result<()> {
    let link_type = link_type.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO memory_links (source_id, target_id, similarity, type) \
             VALUES (?1, ?2, 0.95, ?3)",
            rusqlite::params![source_id, target_id, link_type],
        )?;
        Ok(())
    })
    .await
}

/// Set a memory's created_at so temporal scans have deterministic ordering.
async fn set_created(db: &Database, id: i64, ts: &str) {
    let ts = ts.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET created_at = ?1 WHERE id = ?2",
            rusqlite::params![ts, id],
        )?;
        Ok(())
    })
    .await
    .expect("set created_at");
}

/// time_travel must only return the caller's memories in shared-DB mode.
#[tokio::test]
async fn time_travel_is_scoped_to_owner() {
    let db = monolith().await;
    let _alice = store(&db, "alice time capsule", "general", 10).await;
    let _bob = store(&db, "bob secret capsule", "general", 20).await;

    let alice_view = time_travel(&db, 10, None, "2999-01-01 00:00:00", 100)
        .await
        .expect("time travel");
    assert!(alice_view.iter().any(|r| r.content.contains("alice")));
    assert!(
        !alice_view.iter().any(|r| r.content.contains("bob")),
        "time_travel must not surface another user's memories"
    );

    // Query-filtered branch is scoped too.
    let alice_q = time_travel(&db, 10, Some("capsule"), "2999-01-01 00:00:00", 100)
        .await
        .expect("time travel query");
    assert!(!alice_q.iter().any(|r| r.content.contains("bob")));
}

/// detect_patterns must only scan the caller's memories.
#[tokio::test]
async fn detect_patterns_is_scoped_to_owner() {
    let db = monolith().await;
    // Only Bob has a pattern-worthy set; Alice owns nothing.
    for i in 0..8 {
        let id = store(&db, &format!("bob event {i}"), "ops", 20).await;
        set_created(&db, id, &format!("2026-04-0{} 10:00:00", (i % 9) + 1)).await;
    }
    let alice_patterns = detect_patterns(&db, 10).await.expect("detect patterns");
    assert!(
        alice_patterns.is_empty(),
        "owner with no memories must see no patterns mined from another user"
    );
}

/// detect_sequence_patterns must only mine the caller's memory timeline.
#[tokio::test]
async fn detect_sequence_patterns_is_scoped_to_owner() {
    let db = monolith().await;
    // Bob has an alternating code/docs cadence; Alice owns nothing.
    let seeds = [
        ("bob code one", "code", "2026-04-01 10:00:00"),
        ("bob docs one", "docs", "2026-04-01 10:05:00"),
        ("bob code two", "code", "2026-04-01 11:00:00"),
        ("bob docs two", "docs", "2026-04-01 11:05:00"),
    ];
    for (content, cat, ts) in seeds {
        let id = store(&db, content, cat, 20).await;
        set_created(&db, id, ts).await;
    }
    let alice = detect_sequence_patterns(&db, 10, 30)
        .await
        .expect("sequence patterns");
    assert!(
        alice.is_empty(),
        "owner with no memories must mine no sequences from another user"
    );
}

/// list_observations and materialize must be scoped to the owner.
#[tokio::test]
async fn growth_observations_and_materialize_are_scoped() {
    let db = monolith().await;
    let _alice_obs = store(&db, "alice growth note", "growth", 10).await;
    let bob_obs = store(&db, "bob growth note", "growth", 20).await;

    let alice_list = list_observations(&db, 10, 100).await.expect("list");
    assert!(alice_list.iter().any(|o| o.content.contains("alice")));
    assert!(
        !alice_list.iter().any(|o| o.content.contains("bob")),
        "owner must not see another user's growth observations"
    );

    // Alice cannot materialize Bob's observation.
    assert!(
        materialize(&db, bob_obs, 10).await.is_err(),
        "cross-user materialize must fail closed"
    );
}

/// find_duplicates and find_consolidation_candidates must exclude pairs that
/// touch another user's memories even when a cross-user link row exists.
#[tokio::test]
async fn duplicate_and_consolidation_candidates_exclude_cross_user_links() {
    let db = monolith().await;
    let a1 = store(&db, "alice dup one", "general", 10).await;
    let a2 = store(&db, "alice dup two", "general", 10).await;
    let b1 = store(&db, "bob dup one", "general", 20).await;

    // Same-owner similarity link is allowed.
    raw_link(&db, a1, a2, "similarity")
        .await
        .expect("same-owner link should be allowed");
    // A cross-user link is rejected at the database level by the
    // prevent_cross_tenant_links trigger, so cross-user duplicate/consolidation
    // pairs can never enter the link graph in the first place.
    assert!(
        raw_link(&db, a1, b1, "similarity").await.is_err(),
        "cross-user link must be rejected by prevent_cross_tenant_links"
    );

    // The owner's same-user pair still surfaces, and no result references the
    // other user's memory (belt-and-suspenders with the trigger).
    let dups = find_duplicates(&db, 10, 0.5, 100)
        .await
        .expect("duplicates");
    assert!(
        dups.iter().all(|p| p.id_a != b1 && p.id_b != b1),
        "find_duplicates must not pair across users"
    );
    assert!(
        dups.iter()
            .any(|p| (p.id_a == a1 && p.id_b == a2) || (p.id_a == a2 && p.id_b == a1)),
        "same-owner duplicate pair should still surface"
    );

    let groups = find_consolidation_candidates(&db, 0.5, 10)
        .await
        .expect("consolidation candidates");
    assert!(
        groups.iter().all(|g| !g.contains(&b1.to_string())),
        "consolidation candidates must not include another user's memory"
    );
}
