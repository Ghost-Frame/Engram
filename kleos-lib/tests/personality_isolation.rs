//! Shared-DB isolation tests for personality profile synthesis.
//!
//! These tests run against the monolith schema (`ENGRAM_TENANT_SHARDING=0`)
//! where row-level `user_id` predicates are the only tenant boundary. They
//! prove that `synthesize_personality_profile` only draws on the calling
//! tenant's structured facts and static memories.

use kleos_lib::db::Database;
use kleos_lib::personality::synthesize_personality_profile;
use rusqlite::params;

/// Build a shared monolith database with the full migration chain applied.
async fn monolith() -> Database {
    Database::connect_memory().await.expect("monolith db")
}

/// Insert a personality signal so synthesis clears its "insufficient data" gate.
async fn insert_signal(db: &Database, user_id: i64, subject: &str) {
    let subject = subject.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO personality_signals (signal_type, subject, valence, value, intensity, user_id) \
             VALUES ('preference', ?1, 'positive', 0.9, 0.9, ?2)",
            params![subject, user_id],
        )?;
        Ok(())
    })
    .await
    .expect("insert personality signal");
}

/// Insert a structured fact owned by `user_id`.
async fn insert_fact(db: &Database, user_id: i64, subject: &str, verb: &str, object: &str) {
    let (subject, verb, object) = (subject.to_string(), verb.to_string(), object.to_string());
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO structured_facts (subject, predicate, object, verb, user_id) \
             VALUES (?1, ?2, ?3, ?2, ?4)",
            params![subject, verb, object, user_id],
        )?;
        Ok(())
    })
    .await
    .expect("insert structured fact");
}

/// Insert a static memory owned by `user_id`.
async fn insert_static_memory(db: &Database, user_id: i64, content: &str) {
    let content = content.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO memories (content, importance, is_static, is_forgotten, user_id) \
             VALUES (?1, 9, 1, 0, ?2)",
            params![content, user_id],
        )?;
        Ok(())
    })
    .await
    .expect("insert static memory");
}

/// A profile synthesized for one tenant must not contain another tenant's
/// structured facts or static memories.
#[tokio::test]
async fn synthesis_excludes_other_tenants_facts_and_memories() {
    let db = monolith().await;

    // User 1 has a signal (to pass the gate) plus a distinctive fact and memory.
    insert_signal(&db, 1, "tenant_one_topic").await;
    insert_fact(&db, 1, "tenant_one_subject", "likes", "tenant_one_object").await;
    insert_static_memory(&db, 1, "TENANT_ONE_IDENTITY_MARKER").await;

    // User 2 has their own distinctive fact and memory that must never leak.
    insert_fact(
        &db,
        2,
        "tenant_two_subject",
        "likes",
        "TENANT_TWO_FACT_LEAK",
    )
    .await;
    insert_static_memory(&db, 2, "TENANT_TWO_IDENTITY_LEAK").await;

    let profile = synthesize_personality_profile(&db, 1)
        .await
        .expect("synthesize profile for user 1");

    // User 1's own data is present.
    assert!(
        profile.contains("TENANT_ONE_IDENTITY_MARKER"),
        "profile should include the caller's own static memory"
    );

    // User 2's data must NOT appear in user 1's profile.
    assert!(
        !profile.contains("TENANT_TWO_IDENTITY_LEAK"),
        "profile leaked another tenant's static memory"
    );
    assert!(
        !profile.contains("TENANT_TWO_FACT_LEAK"),
        "profile leaked another tenant's structured fact"
    );
}

/// A tenant with a signal but no facts or static memories yields a profile that
/// contains none of another tenant's facts or memories.
#[tokio::test]
async fn synthesis_for_empty_tenant_does_not_borrow_other_data() {
    let db = monolith().await;

    // User 1 has only a signal: no facts, no static memories.
    insert_signal(&db, 1, "lonely_topic").await;

    // User 2 has facts and memories that must stay private.
    insert_fact(
        &db,
        2,
        "tenant_two_subject",
        "likes",
        "TENANT_TWO_FACT_LEAK",
    )
    .await;
    insert_static_memory(&db, 2, "TENANT_TWO_IDENTITY_LEAK").await;

    let profile = synthesize_personality_profile(&db, 1)
        .await
        .expect("synthesize profile for user 1");

    assert!(
        !profile.contains("TENANT_TWO_IDENTITY_LEAK"),
        "empty tenant's profile leaked another tenant's static memory"
    );
    assert!(
        !profile.contains("TENANT_TWO_FACT_LEAK"),
        "empty tenant's profile leaked another tenant's structured fact"
    );
}
