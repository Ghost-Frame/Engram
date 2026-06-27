//! Integration tests for L5 retrieval channels: the structured_facts FTS channel
//! (current-truth + fail-closed isolation) and the episode LIKE->FTS5 fix.
//!
//! Uses an in-memory DB (`connect_memory` runs the full migration chain, so `structured_facts`,
//! `facts_fts`, `episodes`, and `episodes_fts` all exist with their sync triggers).

use kleos_lib::db::Database;
use kleos_lib::memory::facts_channel::{misscoped_facts_count, search_facts_fts};
use kleos_lib::memory::fts::fts_or_match_query;
use kleos_lib::memory::types::StoreRequest;
use kleos_lib::{episodes, memory};
use rusqlite::params;

/// Store a bare memory (no embedding; FTS/structured path only) and return its id.
async fn store_memory(db: &Database, content: &str, user_id: i64) -> i64 {
    let req = StoreRequest {
        content: content.to_string(),
        category: "fact".to_string(),
        source: "test".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        chunk_embeddings: None,
        session_id: None,
        is_static: Some(false),
        user_id: Some(user_id),
        space_id: None,
        parent_memory_id: None,
        sync_id: None,
        artifacts: None,
    };
    memory::store(db, req, None, false)
        .await
        .expect("store memory")
        .id
}

/// Insert one structured_facts row (the AFTER INSERT trigger syncs facts_fts automatically).
async fn insert_fact(
    db: &Database,
    memory_id: i64,
    subject: &str,
    predicate: &str,
    object: &str,
    user_id: i64,
    invalid_at: Option<&str>,
) {
    let (subject, predicate, object) = (
        subject.to_string(),
        predicate.to_string(),
        object.to_string(),
    );
    let invalid_at = invalid_at.map(str::to_string);
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO structured_facts (memory_id, subject, predicate, object, verb, user_id, invalid_at) \
             VALUES (?1, ?2, ?3, ?4, 'is', ?5, ?6)",
            params![memory_id, subject, predicate, object, user_id, invalid_at],
        )?;
        Ok(())
    })
    .await
    .expect("insert structured_fact");
}

#[tokio::test]
async fn facts_channel_returns_parent_memory_for_current_fact() {
    let db = Database::connect_memory().await.expect("connect_memory");
    let mem = store_memory(&db, "Notes about where things live", 1).await;
    insert_fact(&db, mem, "Borealis", "deployedOn", "Nimbus", 1, None).await;

    let hits = search_facts_fts(&db, &fts_or_match_query("Borealis deployed"), 1, 10)
        .await
        .expect("facts search");
    assert!(
        hits.iter().any(|h| h.memory_id == mem),
        "a query matching a current fact must surface its parent memory; got {hits:?}"
    );
}

#[tokio::test]
async fn facts_channel_excludes_invalidated_facts() {
    let db = Database::connect_memory().await.expect("connect_memory");
    let mem = store_memory(&db, "Stale fact source", 1).await;
    // Superseded fact: invalid_at set -> must not surface as current truth.
    insert_fact(
        &db,
        mem,
        "Calyx",
        "livesIn",
        "Berlin",
        1,
        Some("2025-01-15T00:00:00Z"),
    )
    .await;

    let hits = search_facts_fts(&db, &fts_or_match_query("Calyx Berlin"), 1, 10)
        .await
        .expect("facts search");
    assert!(
        hits.is_empty(),
        "an invalidated (non-current) fact must not be returned; got {hits:?}"
    );
}

#[tokio::test]
async fn facts_channel_is_fail_closed_on_misscoped_rows() {
    let db = Database::connect_memory().await.expect("connect_memory");
    // Memory belongs to user 1; a mis-scoped fact claims user 999.
    let mem = store_memory(&db, "Owner-1 memory", 1).await;
    insert_fact(&db, mem, "Vexel", "uses", "SQLite", 999, None).await;

    // Invisible to the memory's real owner (sf.user_id != 1)...
    let as_owner = search_facts_fts(&db, &fts_or_match_query("Vexel SQLite"), 1, 10)
        .await
        .expect("facts search owner");
    assert!(
        as_owner.is_empty(),
        "mis-scoped fact must be invisible to the memory owner"
    );

    // ...and to the claimed owner (parent memory m.user_id != 999).
    let as_claimer = search_facts_fts(&db, &fts_or_match_query("Vexel SQLite"), 999, 10)
        .await
        .expect("facts search claimer");
    assert!(
        as_claimer.is_empty(),
        "mis-scoped fact must be invisible to the claimed owner"
    );

    // The guard helper still flags the row for ops/data-hygiene.
    let n = misscoped_facts_count(&db).await.expect("misscoped count");
    assert_eq!(
        n, 1,
        "misscoped_facts_count must detect the user_id mismatch"
    );
}

#[tokio::test]
async fn episode_fts_matches_and_isolates_by_user() {
    let db = Database::connect_memory().await.expect("connect_memory");
    // Two users with episodes that share a distinctive token.
    let e1 = episodes::create_episode(
        &db,
        episodes::CreateEpisodeRequest {
            title: Some("Photon gateway rollout".to_string()),
            session_id: None,
            agent: Some("ops".to_string()),
            summary: Some("Configured the photon gateway across services".to_string()),
        },
        1,
    )
    .await
    .expect("create episode u1");
    episodes::create_episode(
        &db,
        episodes::CreateEpisodeRequest {
            title: Some("Photon notes for tenant two".to_string()),
            session_id: None,
            agent: Some("ops".to_string()),
            summary: Some("Other tenant photon summary".to_string()),
        },
        2,
    )
    .await
    .expect("create episode u2");

    // User 1's FTS search matches their episode via the index (not a LIKE scan)...
    let hits = episodes::search_episodes_fts(&db, 1, "photon gateway", 10)
        .await
        .expect("episode fts");
    assert!(
        hits.iter().any(|e| e.id == e1.id),
        "FTS query must surface the matching episode; got {hits:?}"
    );
    // ...and never another tenant's episode.
    assert!(
        hits.iter().all(|e| e.user_id == 1),
        "episode FTS must be user-scoped; got owners {:?}",
        hits.iter().map(|e| e.user_id).collect::<Vec<_>>()
    );

    // Garbage / stopword-only input returns empty, not an error.
    let none = episodes::search_episodes_fts(&db, 1, "...", 10)
        .await
        .expect("episode fts empty");
    assert!(
        none.is_empty(),
        "no usable token -> empty result, not an error"
    );
}

/// L4b data path: detect_communities (env-tunable cap) assigns a community_id to every
/// memory and get_community_members -- the community channel's source -- returns them, user-scoped.
#[tokio::test]
async fn community_detection_populates_and_members_are_scoped() {
    use kleos_lib::graph::communities::{detect_communities, get_community_members};
    let db = Database::connect_memory().await.expect("connect_memory");
    let m1 = store_memory(&db, "graph cluster node one", 1).await;
    let m2 = store_memory(&db, "graph cluster node two", 1).await;
    // A different owner's memory must never appear in user 1's communities.
    let other = store_memory(&db, "another tenant note", 2).await;

    let res = detect_communities(&db, 1, 25).await.expect("detect");
    assert!(
        res.memories >= 2,
        "both user-1 memories clustered; got {res:?}"
    );

    let covered: i64 = db
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM memories WHERE user_id = 1 AND community_id IS NOT NULL",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(covered, 2, "detection populated community_id for user 1");

    let cid: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT community_id FROM memories WHERE id = ?1",
                params![m1],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    let members = get_community_members(&db, cid, 1, 10)
        .await
        .expect("members");
    assert!(members.iter().any(|mm| mm.id == m1 || mm.id == m2));
    assert!(
        !members.iter().any(|mm| mm.id == other),
        "get_community_members must be user-scoped"
    );
}
