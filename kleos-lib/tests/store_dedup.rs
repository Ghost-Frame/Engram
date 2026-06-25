//! Regression tests for store-path near-duplicate scoping (Phase 1.6).
//!
//! Near-duplicate detection must collapse genuine duplicates within one space, but must
//! NOT collapse the same content stored in a different space, nor short-circuit an
//! explicit version update (parent_memory_id set) into its predecessor.

use kleos_lib::db::Database;
use kleos_lib::memory;
use kleos_lib::memory::simhash;
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

/// A near-duplicate (small edit, simhash hamming distance in the open band 0 < d < 3)
/// collapses into the original. This guards the dedup *threshold* itself: the bit-identical
/// test above still passes if a regression tightens `< 3` to `== 0`, but this one fails,
/// because a near-duplicate would then slip through as a new memory.
#[tokio::test]
async fn same_space_near_duplicate_within_band_collapses() {
    // A long base sentence; perturb it minimally until a variant lands inside the dedup band.
    // Searching at runtime keeps the test honest if the simhash implementation ever changes:
    // it self-validates the precondition rather than hard-coding a fragile pair.
    let base = "the quarterly planning review covers staffing budget roadmap and the migration \
                timeline for the analytics platform across both regions";
    let base_hash = simhash::simhash(base);
    // Each candidate appends one short token, the smallest perturbation that still alters content.
    let near = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa",
        "lambda", "sigma", "omega", "tau",
    ]
    .iter()
    .map(|w| format!("{base} {w}"))
    .find(|cand| {
        let d = simhash::hamming_distance(base_hash, simhash::simhash(cand));
        d > 0 && d < 3
    })
    .expect(
        "a one-token perturbation must land in the (0,3) dedup band; update if simhash changed",
    );

    let db = Database::connect_memory().await.expect("db");
    let a = store(&db, base, None, None).await;
    let b = store(&db, &near, None, None).await;
    assert!(a.created, "first store creates");
    assert!(
        !b.created,
        "a near-duplicate within the simhash band must be deduped"
    );
    assert_eq!(b.duplicate_of, Some(a.id));
}

/// Clearly distinct content in the same space is NOT deduped. Guards the threshold from the
/// other side: a regression that loosened `< 3` to a large band would collapse unrelated
/// memories and fail this test.
#[tokio::test]
async fn same_space_distinct_content_not_deduped() {
    let db = Database::connect_memory().await.expect("db");
    let a = store(
        &db,
        "wireguard builds an encrypted tunnel using public key cryptography",
        None,
        None,
    )
    .await;
    let b = store(
        &db,
        "postgres uses a write ahead log for durability of committed transactions",
        None,
        None,
    )
    .await;
    assert!(a.created, "first store creates");
    assert!(
        b.created,
        "unrelated content must not be deduped against the prior memory"
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
