//! Consolidation semantics regressions for Batch F.
//!
//! These tests pin the intended contract:
//! - consolidated source rows leave active prompt, search, and context surfaces;
//! - direct owner fetch by id still works for consolidated source rows;
//! - consolidation lineage stays available through explicit audit APIs;
//! - mixed-owner consolidation requests fail closed.

use kleos_lib::context::deps::{get_links, get_recent_dynamic};
use kleos_lib::db::Database;
use kleos_lib::intelligence::consolidation::consolidate;
use kleos_lib::memory;
use kleos_lib::memory::search::hybrid_search;
use kleos_lib::memory::types::{SearchRequest, StoreRequest};
use kleos_lib::prompts::generate_prompt;

/// Build a fresh in-memory monolith database for consolidation semantics tests.
async fn monolith() -> Database {
    Database::connect_memory().await.expect("monolith db")
}

/// Build a minimal store request for one owned test memory.
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
        is_static: Some(false),
        user_id: Some(user_id),
        space_id: None,
        parent_memory_id: None,
        sync_id: None,
        artifacts: None,
    }
}

/// Persist one owned test memory and return its row id.
async fn store_memory(db: &Database, content: &str, user_id: i64) -> i64 {
    memory::store(db, store_req(content, user_id), None, false)
        .await
        .expect("store memory")
        .id
}

/// Consolidated sources should leave active surfaces while lineage stays auditable.
#[tokio::test]
async fn consolidated_sources_leave_active_surfaces_but_keep_lineage() {
    let db = monolith().await;
    let user_id = 11;

    let source_a = store_memory(&db, "batch-f alpha needle", user_id).await;
    let source_b = store_memory(&db, "batch-f beta needle", user_id).await;

    let summary = consolidate(&db, &[source_a.to_string(), source_b.to_string()], user_id)
        .await
        .expect("consolidate");

    let prompt = generate_prompt(&db, "openai", 4_000, "ctx", user_id)
        .await
        .expect("generate prompt");
    assert_eq!(
        prompt.memories_included, 1,
        "prompt surface must include only the consolidated summary row"
    );

    let search = hybrid_search(
        &db,
        SearchRequest {
            query: "batch-f alpha needle".to_string(),
            user_id: Some(user_id),
            limit: Some(10),
            ..Default::default()
        },
    )
    .await
    .expect("search");
    let search_ids: Vec<i64> = search.iter().map(|result| result.memory.id).collect();
    assert!(
        !search_ids.contains(&source_a),
        "search surface must hide consolidated source rows"
    );

    let recent = get_recent_dynamic(&db, user_id, 10)
        .await
        .expect("recent dynamic");
    let recent_ids: Vec<i64> = recent.iter().map(|memory| memory.id).collect();
    assert!(
        !recent_ids.contains(&source_a) && !recent_ids.contains(&source_b),
        "active context surface must hide consolidated source rows"
    );

    let active_links = get_links(&db, summary.id, user_id)
        .await
        .expect("active links");
    assert!(
        active_links.is_empty(),
        "active context traversal must keep consolidation lineage hidden"
    );

    let lineage = memory::get_links_for(&db, summary.id, user_id)
        .await
        .expect("lineage links");
    let lineage_ids: Vec<i64> = lineage.iter().map(|link| link.id).collect();
    assert!(
        lineage_ids.contains(&source_a) && lineage_ids.contains(&source_b),
        "audit lineage must still expose consolidated source ids"
    );
}

/// Direct owner fetch should still work for a consolidated source row by id.
#[tokio::test]
async fn consolidated_source_stays_fetchable_by_owner_id() {
    let db = monolith().await;
    let user_id = 12;

    let source_a = store_memory(&db, "batch-f direct fetch alpha", user_id).await;
    let source_b = store_memory(&db, "batch-f direct fetch beta", user_id).await;

    consolidate(&db, &[source_a.to_string(), source_b.to_string()], user_id)
        .await
        .expect("consolidate");

    let fetched = memory::get(&db, source_a, user_id)
        .await
        .expect("direct fetch");
    assert_eq!(fetched.id, source_a, "owner fetch must still resolve by id");
    assert!(
        fetched.is_consolidated,
        "direct fetch should return the consolidated source row"
    );
    assert!(
        !fetched.is_latest,
        "consolidated source row must no longer be latest"
    );
}

/// Mixed-owner consolidation requests should fail and leave owned rows active.
#[tokio::test]
async fn consolidate_rejects_mixed_owner_ids() {
    let db = monolith().await;

    let owner_a = store_memory(&db, "batch-f owner-a", 21).await;
    let other_user = store_memory(&db, "batch-f owner-b", 22).await;

    let result = consolidate(&db, &[owner_a.to_string(), other_user.to_string()], 21).await;
    assert!(
        result.is_err(),
        "mixed-owner consolidation must fail closed"
    );

    let owner_memory = memory::get(&db, owner_a, 21)
        .await
        .expect("owner memory still fetchable");
    assert!(
        !owner_memory.is_consolidated,
        "failed consolidation must not mutate the owned source row"
    );
    assert!(
        owner_memory.is_latest,
        "failed consolidation must leave the owned source row latest"
    );
}
