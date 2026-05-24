//! Integration tests for entity extraction wiring in the Kleos ingest pipeline.
//!
//! These tests verify that `extract_and_link_entities` correctly populates
//! the `entities`, `memory_entities`, and `entity_cooccurrences` tables when
//! called against a live tenant database. They act as the TDD gate for the
//! entity-wiring work: write them first (they pass trivially against direct
//! calls), then assert the higher-level pipeline actually calls extraction.
//!
//! Each test spins up a fresh tenant database via `TenantRegistry` so there is
//! no shared state between test runs.

use std::sync::Arc;
use tempfile::tempdir;

use kleos_lib::tenant::{TenantConfig, TenantHandle, TenantRegistry};

/// Spin up a single isolated tenant handle backed by a temporary directory.
async fn single_tenant() -> Arc<TenantHandle> {
    let dir = tempdir().expect("tempdir");
    let registry = TenantRegistry::new(dir.path(), TenantConfig::default(), 128, false, None)
        .expect("registry");

    let handle = registry.get_or_create("test_tenant").await.expect("tenant");

    // Leak dir so the backing files stay alive for the duration of the test.
    std::mem::forget(dir);
    handle
}

// ---------------------------------------------------------------------------
// Positive case: capitalized named entities are extracted and linked
// ---------------------------------------------------------------------------

/// Store a memory with named entities then call `extract_and_link_entities`
/// directly.
///
/// Asserts:
///   - `entities` table contains a row for "Tim Cook"
///   - `entities` table contains a row for "Apple Inc"
///   - `memory_entities` has two link rows pointing to the stored memory
///   - `entity_cooccurrences` has one row for the (Tim Cook, Apple Inc) pair
#[tokio::test]
async fn extract_and_link_entities_populates_tables() {
    use kleos_lib::graph::entities::extract_and_link_entities;
    use kleos_lib::memory::{self, types::StoreRequest};

    let tenant = single_tenant().await;
    let db = tenant.database();

    // Insert a memory so we have a valid memory_id.
    // Use content where both multi-word capitalized phrases are followed by
    // lowercase words so the heuristic does not extend the run past the
    // intended boundary.
    let content = "Tim Cook works at Apple Inc, which launched a product yesterday.";
    let store_req = StoreRequest {
        content: content.to_string(),
        category: "general".to_string(),
        source: "test".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        session_id: None,
        is_static: None,
        user_id: Some(1),
        space_id: None,
        parent_memory_id: None,
        chunk_embeddings: None,
        sync_id: None,
        artifacts: None,
    };
    let stored = memory::store(&db, store_req, None, false)
        .await
        .expect("store memory");
    let memory_id = stored.id;

    // Call the function under test directly (no spawn indirection needed here).
    let entities = extract_and_link_entities(&db, memory_id, content, 1)
        .await
        .expect("extract_and_link_entities");

    // At least Tim Cook and Apple Inc should be found.
    let names: Vec<String> = entities.iter().map(|e| e.name.clone()).collect();
    assert!(
        names.iter().any(|n| n == "Tim Cook"),
        "expected Tim Cook in entities, got: {:?}",
        names
    );
    assert!(
        names.iter().any(|n| n == "Apple Inc"),
        "expected Apple Inc in entities, got: {:?}",
        names
    );

    // memory_entities link table must have rows for this memory.
    let link_count: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_entities WHERE memory_id = ?1",
                rusqlite::params![memory_id],
                |row| row.get(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("link count");
    assert!(
        link_count >= 2,
        "expected >=2 memory_entity links, got {}",
        link_count
    );

    // entity_cooccurrences must have at least one row (Tim Cook <-> Apple Inc).
    let cooc_count: i64 = db
        .read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM entity_cooccurrences", [], |row| {
                row.get(0)
            })
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("cooccurrence count");
    assert!(
        cooc_count >= 1,
        "expected >=1 cooccurrence row, got {}",
        cooc_count
    );
}

// ---------------------------------------------------------------------------
// Negative case: all-lowercase content yields zero entities
// ---------------------------------------------------------------------------

/// When content contains no capitalized words, `extract_and_link_entities`
/// must create zero entity rows and zero link rows.
#[tokio::test]
async fn extract_and_link_entities_skips_lowercase_content() {
    use kleos_lib::graph::entities::extract_and_link_entities;
    use kleos_lib::memory::{self, types::StoreRequest};

    let tenant = single_tenant().await;
    let db = tenant.database();

    let store_req = StoreRequest {
        content: "the cat sat on the mat".to_string(),
        category: "general".to_string(),
        source: "test".to_string(),
        importance: 5,
        tags: None,
        embedding: None,
        session_id: None,
        is_static: None,
        user_id: Some(1),
        space_id: None,
        parent_memory_id: None,
        chunk_embeddings: None,
        sync_id: None,
        artifacts: None,
    };
    let stored = memory::store(&db, store_req, None, false)
        .await
        .expect("store memory");
    let memory_id = stored.id;

    let entities = extract_and_link_entities(&db, memory_id, "the cat sat on the mat", 1)
        .await
        .expect("extract_and_link_entities");

    assert!(
        entities.is_empty(),
        "expected zero entities for all-lowercase content, got: {:?}",
        entities.iter().map(|e| &e.name).collect::<Vec<_>>()
    );

    // Confirm link table is also empty for this memory.
    let link_count: i64 = db
        .read(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM memory_entities WHERE memory_id = ?1",
                rusqlite::params![memory_id],
                |row| row.get(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .expect("link count");
    assert_eq!(link_count, 0, "expected 0 links for all-lowercase content");
}
