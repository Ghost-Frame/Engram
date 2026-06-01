//! Shared-DB scoping regressions for per-user skill analysis (Batch I).
//!
//! Master's decision: skill evolution is per-user. These tests run two users
//! against one monolith database and assert that the skill-analysis read paths
//! never observe another user's skills when `ENGRAM_TENANT_SHARDING=0`.
//! `skill_records` carries `user_id` (re-added in migration 78); related
//! analytics tables key off `skill_id`.

use kleos_lib::db::Database;
use kleos_lib::skills::analyzer::{
    correct_skill_id, get_failing_skill_candidates, get_usage_stats,
};
use kleos_lib::skills::list_recent_evolutions;
use kleos_lib::skills::types::CreateSkillRequest;

/// Build a shared monolith in-memory database.
async fn monolith() -> Database {
    Database::connect_memory().await.expect("monolith db")
}

/// Create one skill owned by `user_id`, tagged so it shows in evolution feeds.
async fn make_skill(db: &Database, name: &str, user_id: i64) -> i64 {
    let req = CreateSkillRequest {
        name: name.to_string(),
        agent: "test".to_string(),
        description: Some("scope test skill".to_string()),
        code: "fn run() {}".to_string(),
        language: Some("rust".to_string()),
        parent_skill_id: None,
        metadata: None,
        user_id: Some(user_id),
        tags: Some(vec!["fixed".to_string()]),
        tool_deps: None,
        kind: None,
        source_plugin: None,
        source_path: None,
        content_hash: None,
    };
    kleos_lib::skills::create_skill(db, req)
        .await
        .expect("create skill")
        .id
}

/// Force a skill into the "active but failing" shape so the analysis paths
/// surface it: 10 executions, 1 success, 9 failures.
async fn mark_failing(db: &Database, skill_id: i64) {
    db.write(move |conn| {
        conn.execute(
            "UPDATE skill_records \
             SET is_active = 1, is_deprecated = 0, \
                 execution_count = 10, success_count = 1, failure_count = 9 \
             WHERE id = ?1",
            rusqlite::params![skill_id],
        )?;
        Ok(())
    })
    .await
    .expect("mark failing");
}

/// get_usage_stats and get_failing_skill_candidates must only see the owner's
/// skills.
#[tokio::test]
async fn skill_usage_and_failing_candidates_are_scoped() {
    let db = monolith().await;
    let alice = make_skill(&db, "alice_skill", 10).await;
    let bob = make_skill(&db, "bob_skill", 20).await;
    mark_failing(&db, alice).await;
    mark_failing(&db, bob).await;

    let stats = get_usage_stats(&db, 10).await.expect("usage stats");
    let failing = stats["failing"].as_array().expect("failing array");
    let names: Vec<&str> = failing.iter().filter_map(|e| e["name"].as_str()).collect();
    assert!(
        names.contains(&"alice_skill"),
        "owner skill should be listed"
    );
    assert!(
        !names.contains(&"bob_skill"),
        "another user's failing skill must not appear in owner stats"
    );

    let candidates = get_failing_skill_candidates(&db, 10, 1, 0.5, 0, 100)
        .await
        .expect("failing candidates");
    assert!(candidates.contains(&alice));
    assert!(
        !candidates.contains(&bob),
        "auto-fix candidates must not include another user's skill"
    );
}

/// correct_skill_id resolves fuzzy names only within the caller's skills.
#[tokio::test]
async fn correct_skill_id_is_scoped() {
    let db = monolith().await;
    let _alice = make_skill(&db, "alice_skill", 10).await;
    let _bob = make_skill(&db, "bob_skill", 20).await;

    // Fuzzy match within the owner's skills resolves.
    assert_eq!(
        correct_skill_id(&db, "alice_skil", 10)
            .await
            .expect("correct"),
        Some("alice_skill".to_string())
    );
    // The other user's skill name is not resolvable for this owner.
    assert_eq!(
        correct_skill_id(&db, "bob_skill", 10)
            .await
            .expect("correct"),
        None
    );
}

/// list_recent_evolutions surfaces only the caller's evolved skills.
#[tokio::test]
async fn recent_evolutions_are_scoped() {
    let db = monolith().await;
    let alice = make_skill(&db, "alice_skill", 10).await;
    let bob = make_skill(&db, "bob_skill", 20).await;

    let feed = list_recent_evolutions(&db, 10, 24, 100)
        .await
        .expect("evolutions");
    let ids: Vec<i64> = feed.iter().map(|r| r.skill_id).collect();
    assert!(ids.contains(&alice), "owner evolution should appear");
    assert!(
        !ids.contains(&bob),
        "another user's evolution must not appear"
    );
}
