//! EXPLAIN QUERY PLAN audit (Part 7.6)
//!
//! Ensures the hottest queries in the retrieval + graph paths use indexes
//! rather than falling back to full table scans. Runs SQLite's
//! `EXPLAIN QUERY PLAN` against each query and asserts the resulting plan
//! mentions `USING INDEX` or `USING COVERING INDEX`.
//!
//! When this test fails, either:
//!   (a) an index was dropped/renamed and the query regressed to a scan, or
//!   (b) a new query was added without a covering index.
//!
//! Both cases need review before landing.

use std::sync::Arc;

use engram_lib::db::Database;
use rusqlite::params;

async fn seed_db() -> Arc<Database> {
    let db = Arc::new(Database::connect_memory().await.expect("in-memory db"));
    // Seed a few rows so the planner has realistic cardinality hints. SQLite's
    // planner still uses available indexes even without ANALYZE data, but
    // rows make the assertions meaningful (empty tables sometimes collapse
    // to `SCAN`).
    db.write(|conn| {
        for i in 0..20 {
            conn.execute(
                "INSERT INTO memories (content, category, source, user_id, importance, confidence, created_at, updated_at, is_latest, is_forgotten, is_archived)
                 VALUES (?1, 'general', 'test', 1, 5, 1.0, datetime('now'), datetime('now'), 1, 0, 0)",
                params![format!("content-{i}")],
            )
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
        }
        // A few links for graph plans.
        for s in 1..5 {
            for t in 5..10 {
                conn.execute(
                    "INSERT OR IGNORE INTO memory_links (source_id, target_id, similarity, type, created_at)
                     VALUES (?1, ?2, 0.5, 'similarity', datetime('now'))",
                    params![s, t],
                )
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
            }
        }
        Ok(())
    })
    .await
    .expect("seed db");
    db
}

async fn explain(
    db: &Database,
    sql: String,
    params_json: Vec<rusqlite::types::Value>,
) -> Vec<String> {
    let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(&explain_sql)
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = params_json
            .iter()
            .map(|v| v as &dyn rusqlite::ToSql)
            .collect();
        let rows = stmt
            .query_map(params_ref.as_slice(), |row| row.get::<_, String>(3))
            .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?);
        }
        Ok(out)
    })
    .await
    .expect("explain")
}

fn assert_uses_index(name: &str, plan: &[String]) {
    let joined = plan.join(" | ");
    assert!(
        joined.contains("USING INDEX") || joined.contains("USING COVERING INDEX"),
        "query `{name}` does not use an index. Plan: {joined}"
    );
}

#[tokio::test]
async fn hot_query_hybrid_search_base_uses_index() {
    let db = seed_db().await;
    let plan = explain(
        &db,
        "SELECT id FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_archived = 0 AND is_latest = 1 ORDER BY created_at DESC LIMIT 20".into(),
        vec![rusqlite::types::Value::Integer(1)],
    )
    .await;
    assert_uses_index("hybrid_search base", &plan);
}

#[tokio::test]
async fn hot_query_links_by_target_uses_index() {
    let db = seed_db().await;
    let plan = explain(
        &db,
        "SELECT source_id, target_id, similarity FROM memory_links WHERE target_id = ?1".into(),
        vec![rusqlite::types::Value::Integer(5)],
    )
    .await;
    assert_uses_index("memory_links by target", &plan);
}

#[tokio::test]
async fn hot_query_links_by_source_uses_index() {
    let db = seed_db().await;
    let plan = explain(
        &db,
        "SELECT source_id, target_id, similarity FROM memory_links WHERE source_id = ?1".into(),
        vec![rusqlite::types::Value::Integer(1)],
    )
    .await;
    assert_uses_index("memory_links by source", &plan);
}

#[tokio::test]
async fn hot_query_memories_by_user_uses_index() {
    let db = seed_db().await;
    let plan = explain(
        &db,
        "SELECT id FROM memories WHERE user_id = ?1 ORDER BY created_at DESC LIMIT 50".into(),
        vec![rusqlite::types::Value::Integer(1)],
    )
    .await;
    assert_uses_index("memories by user", &plan);
}

#[tokio::test]
async fn hot_query_memories_by_episode_uses_index() {
    let db = seed_db().await;
    let plan = explain(
        &db,
        "SELECT id FROM memories WHERE episode_id = ?1 AND is_forgotten = 0".into(),
        vec![rusqlite::types::Value::Integer(1)],
    )
    .await;
    assert_uses_index("memories by episode", &plan);
}
