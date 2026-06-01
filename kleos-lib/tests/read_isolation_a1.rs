//! Shared-DB read isolation regressions for Batch A1.
//!
//! These tests run against one monolith database and assert that read-only
//! prompt packing, contradiction scans, and pagerank never observe another
//! user's rows when `ENGRAM_TENANT_SHARDING=0`.

use kleos_lib::db::Database;
use kleos_lib::facts::{self, CreateFactRequest};
use kleos_lib::graph::pagerank::compute_pagerank_for_user;
use kleos_lib::intelligence::contradiction::{detect_contradictions, scan_all_contradictions};
use kleos_lib::memory;
use kleos_lib::memory::types::StoreRequest;
use kleos_lib::pack::{pack_memories, PackFormat};
use kleos_lib::prompts::{generate_header, generate_prompt};

/// Build a monolith in-memory database for shared-DB isolation tests.
async fn monolith() -> Database {
    Database::connect_memory().await.expect("monolith db")
}

/// Build a minimal memory store request owned by `user_id`.
fn store_req(content: &str, user_id: i64, is_static: bool) -> StoreRequest {
    StoreRequest {
        content: content.to_string(),
        category: "general".to_string(),
        source: "test".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        chunk_embeddings: None,
        session_id: None,
        is_static: Some(is_static),
        user_id: Some(user_id),
        space_id: None,
        parent_memory_id: None,
        sync_id: None,
        artifacts: None,
    }
}

/// Persist a memory and return its ID.
async fn store_memory(db: &Database, content: &str, user_id: i64, is_static: bool) -> i64 {
    memory::store(db, store_req(content, user_id, is_static), None, false)
        .await
        .expect("store memory")
        .id
}

/// Stamp a model string onto an existing memory so header generation can
/// distinguish prior work by model.
async fn set_memory_model(db: &Database, memory_id: i64, model: &str) {
    let model_owned = model.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE memories SET model = ?1 WHERE id = ?2",
            rusqlite::params![model_owned, memory_id],
        )?;
        Ok(())
    })
    .await
    .expect("set memory model");
}

/// Prompt generation and header generation must only read the caller's rows in
/// a shared monolith database.
#[tokio::test]
async fn prompt_and_header_read_isolation_single_db() {
    let db = monolith().await;

    let alice_id = store_memory(&db, "alice prompt memory", 10, true).await;
    let _bob_id = store_memory(&db, "bob prompt leak", 20, true).await;

    set_memory_model(&db, alice_id, "alice-model").await;
    let bob_for_header = store_memory(&db, "bob header leak", 20, false).await;
    set_memory_model(&db, bob_for_header, "bob-model").await;

    let prompt = generate_prompt(&db, "openai", 4_000, "ctx", 10)
        .await
        .expect("generate prompt");
    assert!(
        prompt.prompt.contains("alice prompt memory"),
        "owner prompt must include owner memory"
    );
    assert!(
        !prompt.prompt.contains("bob prompt leak"),
        "owner prompt must exclude other-user memory"
    );

    let header = generate_header(&db, "current-model", "security-review", "ctx", 5, 10)
        .await
        .expect("generate header");
    assert!(
        header
            .prior_models
            .iter()
            .any(|model| model == "alice-model"),
        "owner header must include owner prior model"
    );
    assert!(
        !header.prior_models.iter().any(|model| model == "bob-model"),
        "owner header must exclude other-user prior model"
    );
    assert!(
        !header.text.contains("bob header leak"),
        "owner header summary must exclude other-user memory content"
    );
}

/// Memory packing must only include the caller's static and high-importance
/// memories in a shared monolith database.
#[tokio::test]
async fn pack_read_isolation_single_db() {
    let db = monolith().await;

    store_memory(&db, "alice static keep", 10, true).await;
    store_memory(&db, "alice important keep", 10, false).await;
    store_memory(&db, "bob static leak", 20, true).await;
    store_memory(&db, "bob important leak", 20, false).await;

    let packed = pack_memories(&db, "ctx", 4_000, PackFormat::Text, 10)
        .await
        .expect("pack memories");
    assert!(
        packed.packed.contains("alice static keep"),
        "owner pack must include owner static memory"
    );
    assert!(
        packed.packed.contains("alice important keep"),
        "owner pack must include owner important memory"
    );
    assert!(
        !packed.packed.contains("bob static leak"),
        "owner pack must exclude other-user static memory"
    );
    assert!(
        !packed.packed.contains("bob important leak"),
        "owner pack must exclude other-user important memory"
    );
}

/// Contradiction detection and full contradiction scans must never compare
/// facts across users in a shared monolith database.
#[tokio::test]
async fn contradiction_read_isolation_single_db() {
    let db = monolith().await;

    let alice_memory_id = store_memory(&db, "alice contradiction source", 10, false).await;
    let bob_memory_id = store_memory(&db, "bob contradiction source", 20, false).await;

    facts::create_fact(
        &db,
        CreateFactRequest {
            memory_id: Some(alice_memory_id),
            subject: "sky".to_string(),
            predicate: "color".to_string(),
            object: "blue".to_string(),
            confidence: Some(0.9),
        },
        10,
    )
    .await
    .expect("create alice fact");
    facts::create_fact(
        &db,
        CreateFactRequest {
            memory_id: Some(bob_memory_id),
            subject: "sky".to_string(),
            predicate: "color".to_string(),
            object: "green".to_string(),
            confidence: Some(0.8),
        },
        20,
    )
    .await
    .expect("create bob fact");

    let alice_memory = memory::get(&db, alice_memory_id, 10)
        .await
        .expect("load alice memory");
    let contradictions = detect_contradictions(&db, &alice_memory)
        .await
        .expect("detect contradictions");
    assert!(
        contradictions.is_empty(),
        "owner contradiction detect must ignore other-user facts"
    );

    let scanned = scan_all_contradictions(&db, 10)
        .await
        .expect("scan contradictions");
    assert!(
        scanned.is_empty(),
        "owner contradiction scan must ignore other-user facts"
    );
}

/// PageRank must only load nodes owned by the requested user in a shared
/// monolith database.
#[tokio::test]
async fn pagerank_read_isolation_single_db() {
    let db = monolith().await;

    let alice_left = store_memory(&db, "alice pagerank left", 10, false).await;
    let alice_right = store_memory(&db, "alice pagerank right", 10, false).await;
    let bob_only = store_memory(&db, "bob pagerank outsider", 20, false).await;

    memory::insert_link(&db, alice_left, alice_right, 1.0, "causes", 10)
        .await
        .expect("insert alice link");

    let scores = compute_pagerank_for_user(&db, 10)
        .await
        .expect("compute pagerank");
    let score_ids: Vec<i64> = scores.iter().map(|(memory_id, _)| *memory_id).collect();

    assert!(
        score_ids.contains(&alice_left) && score_ids.contains(&alice_right),
        "owner pagerank must include owner nodes"
    );
    assert!(
        !score_ids.contains(&bob_only),
        "owner pagerank must exclude other-user nodes"
    );
    assert_eq!(
        score_ids.len(),
        2,
        "owner pagerank must not dilute with other users"
    );
}
