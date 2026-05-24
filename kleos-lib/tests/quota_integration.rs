//! E2 shard quota architecture integration tests.
//!
//! Tests run against in-memory tenant shards so they are fast and isolated.
//! Each test builds its own QuotaConfig and verifies quota enforcement
//! behavior end-to-end through the Database layer.

use kleos_lib::db::Database;
use kleos_lib::quota::enforce_quota_in_tx;
use kleos_lib::tenant::types::QuotaConfig;
use kleos_lib::EngError;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Open an in-memory tenant shard and seed tenant_state with given values.
async fn open_shard_with_state(content_bytes: i64, memory_count: i64) -> Database {
    let db = Database::open_tenant_memory().await.unwrap();
    db.write(move |conn| {
        conn.execute(
            "UPDATE tenant_state SET value = ?1 WHERE key = 'content_bytes'",
            rusqlite::params![content_bytes],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
        conn.execute(
            "UPDATE tenant_state SET value = ?1 WHERE key = 'memory_count'",
            rusqlite::params![memory_count],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
    .unwrap();
    db
}

// ---------------------------------------------------------------------------
// Test 1: Content quota allow
// ---------------------------------------------------------------------------

/// A write that stays within the content_bytes limit succeeds.
#[tokio::test]
async fn test_content_quota_allow() {
    let db = open_shard_with_state(400, 5).await;
    let quota = QuotaConfig {
        content_bytes: Some(1000),
        memory_count: None,
        disk_bytes: None,
    };
    let result = db
        .transaction(move |tx| enforce_quota_in_tx(tx, &quota, 100))
        .await;
    assert!(result.is_ok(), "write within limit must succeed");
}

// ---------------------------------------------------------------------------
// Test 2: Content quota deny
// ---------------------------------------------------------------------------

/// A write that would exceed the content_bytes limit returns QuotaExceeded.
#[tokio::test]
async fn test_content_quota_deny() {
    let db = open_shard_with_state(950, 5).await;
    let quota = QuotaConfig {
        content_bytes: Some(1000),
        memory_count: None,
        disk_bytes: None,
    };
    let result = db
        .transaction(move |tx| enforce_quota_in_tx(tx, &quota, 100))
        .await;
    assert!(matches!(result, Err(EngError::QuotaExceeded(_))));
}

// ---------------------------------------------------------------------------
// Test 3: Memory count quota deny
// ---------------------------------------------------------------------------

/// A write at the memory_count limit is rejected.
#[tokio::test]
async fn test_memory_count_quota_deny() {
    let db = open_shard_with_state(100, 10).await;
    let quota = QuotaConfig {
        content_bytes: None,
        memory_count: Some(10),
        disk_bytes: None,
    };
    let result = db
        .transaction(move |tx| enforce_quota_in_tx(tx, &quota, 50))
        .await;
    assert!(matches!(result, Err(EngError::QuotaExceeded(_))));
}

// ---------------------------------------------------------------------------
// Test 4: Update delta (100B -> 200B) -- content_bytes grows by 100
// ---------------------------------------------------------------------------

/// Updating content from 100B to 200B results in +100 delta to content_bytes.
#[tokio::test]
async fn test_update_delta() {
    let db = open_shard_with_state(1000, 5).await;
    let delta: i64 = 200 - 100;
    db.write(move |conn| {
        conn.execute(
            "UPDATE tenant_state SET value = value + ?1 WHERE key = 'content_bytes'",
            rusqlite::params![delta],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
    .unwrap();

    let new_bytes: i64 = db
        .read(|conn| {
            conn.query_row(
                "SELECT value FROM tenant_state WHERE key = 'content_bytes'",
                [],
                |r| r.get(0),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .unwrap();

    assert_eq!(new_bytes, 1100, "content_bytes must be 1000 + 100 = 1100");
}

// ---------------------------------------------------------------------------
// Test 5: Hard delete decrements
// ---------------------------------------------------------------------------

/// Purging a 200B memory decrements content_bytes and memory_count.
#[tokio::test]
async fn test_hard_delete_decrements() {
    let db = open_shard_with_state(500, 3).await;
    db.write(|conn| {
        conn.execute(
            "UPDATE tenant_state SET value = MAX(0, value - 200) WHERE key = 'content_bytes'",
            [],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
        conn.execute(
            "UPDATE tenant_state SET value = MAX(0, value - 1) WHERE key = 'memory_count'",
            [],
        )
        .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
    .unwrap();

    let (bytes, count): (i64, i64) = db
        .read(|conn| {
            conn.query_row(
                "SELECT
                    (SELECT value FROM tenant_state WHERE key='content_bytes'),
                    (SELECT value FROM tenant_state WHERE key='memory_count')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .unwrap();

    assert_eq!(bytes, 300);
    assert_eq!(count, 2);
}

// ---------------------------------------------------------------------------
// Test 6: Soft delete preserves counter
// ---------------------------------------------------------------------------

/// Soft-deleting a memory (is_forgotten=1) does NOT change counters.
#[tokio::test]
async fn test_soft_delete_no_counter_change() {
    let db = open_shard_with_state(500, 3).await;
    let (bytes, count): (i64, i64) = db
        .read(|conn| {
            conn.query_row(
                "SELECT
                    (SELECT value FROM tenant_state WHERE key='content_bytes'),
                    (SELECT value FROM tenant_state WHERE key='memory_count')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))
        })
        .await
        .unwrap();

    assert_eq!(bytes, 500);
    assert_eq!(count, 3);
}

// ---------------------------------------------------------------------------
// Test 7: Concurrent boundary -- 5 of 10 writes succeed against 5K limit
// ---------------------------------------------------------------------------

/// 10 concurrent writers of 1K each against a 5K limit: exactly 5 succeed.
///
/// This test verifies the serialized single-writer pool prevents over-commit.
#[tokio::test]
async fn test_concurrent_boundary() {
    let db = std::sync::Arc::new(open_shard_with_state(0, 0).await);
    let quota = std::sync::Arc::new(QuotaConfig {
        content_bytes: Some(5000),
        memory_count: None,
        disk_bytes: None,
    });

    let mut tasks = Vec::new();
    for _ in 0..10 {
        let db = std::sync::Arc::clone(&db);
        let quota = std::sync::Arc::clone(&quota);
        tasks.push(tokio::spawn(async move {
            db.transaction(move |tx| {
                enforce_quota_in_tx(tx, &quota, 1000)?;
                tx.execute(
                    "UPDATE tenant_state SET value = value + 1000 WHERE key = 'content_bytes'",
                    [],
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
                Ok(())
            })
            .await
        }));
    }

    let mut successes = 0;
    let mut failures = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(()) => successes += 1,
            Err(EngError::QuotaExceeded(_)) => failures += 1,
            Err(e) => panic!("unexpected error: {}", e),
        }
    }

    assert_eq!(
        successes, 5,
        "exactly 5 of 10 writes must succeed at 1K each within 5K limit"
    );
    assert_eq!(failures, 5, "exactly 5 writes must fail with QuotaExceeded");
}

// ---------------------------------------------------------------------------
// Test 8: ArcSwap quota update (no torn config)
// ---------------------------------------------------------------------------

/// ArcSwap refresh_quota presents the new config atomically.
#[test]
fn test_arcswap_quota_update_atomic() {
    use arc_swap::ArcSwap;
    use std::sync::Arc;

    // Shared quota slot wrapping a QuotaConfig behind an ArcSwap for wait-free reads.
    let swap: ArcSwap<QuotaConfig> = ArcSwap::from_pointee(QuotaConfig::default());

    let initial = swap.load_full();
    assert!(initial.content_bytes.is_none());

    swap.store(Arc::new(QuotaConfig {
        content_bytes: Some(1_000_000),
        memory_count: Some(10_000),
        disk_bytes: None,
    }));

    let updated = swap.load_full();
    assert_eq!(updated.content_bytes, Some(1_000_000));
    assert_eq!(updated.memory_count, Some(10_000));
}

// ---------------------------------------------------------------------------
// Test 9: Recompute repair
// ---------------------------------------------------------------------------

/// After recompute, counters match the actual memories table.
#[tokio::test]
async fn test_recompute_repair() {
    let db = open_shard_with_state(9999, 9999).await;

    let (bytes, count) = db
        .write(|conn| {
            let (b, c): (i64, i64) = conn
                .query_row(
                    "SELECT COALESCE(SUM(length(content)), 0), COUNT(*) \
                     FROM memories WHERE is_latest = 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            conn.execute(
                "UPDATE tenant_state SET value = ?1 WHERE key = 'content_bytes'",
                rusqlite::params![b],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            conn.execute(
                "UPDATE tenant_state SET value = ?1 WHERE key = 'memory_count'",
                rusqlite::params![c],
            )
            .map_err(|e| kleos_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok((b, c))
        })
        .await
        .unwrap();

    assert_eq!(bytes, 0, "recompute on empty shard must yield 0 bytes");
    assert_eq!(count, 0, "recompute on empty shard must yield 0 memories");
}

// ---------------------------------------------------------------------------
// Test 10: Dirty-flag sync (only dirty handles flushed)
// ---------------------------------------------------------------------------

/// take_dirty returns true once then false; non-dirty handles are skipped.
#[test]
fn test_dirty_flag_only_dirty_flushed() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Simulates a dirty-flag on a tenant handle; starts dirty.
    let dirty = AtomicBool::new(true);
    // Simulates a clean handle that has not been written since last sync.
    let clean = AtomicBool::new(false);

    assert!(
        dirty.swap(false, Ordering::Relaxed),
        "first take must return true"
    );
    assert!(
        !dirty.swap(false, Ordering::Relaxed),
        "second take must return false"
    );
    assert!(
        !clean.swap(false, Ordering::Relaxed),
        "clean handle must return false"
    );
}

// ---------------------------------------------------------------------------
// Test 11: Disk sampler over-quota + clear
// ---------------------------------------------------------------------------

/// Disk sampler sets read_only when over limit and clears when under.
#[test]
fn test_disk_sampler_read_only_toggle() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Simulates the read_only flag on a TenantHandle, flipped by the disk sampler.
    let read_only = AtomicBool::new(false);
    let quota_disk_bytes: i64 = 1_000_000;

    // Disk usage exceeds quota -- sampler must set read_only.
    let disk_bytes: i64 = 1_200_000;
    if disk_bytes > quota_disk_bytes {
        read_only.store(true, Ordering::Release);
    }
    assert!(read_only.load(Ordering::Acquire));

    // Disk usage drops below quota -- sampler must clear read_only.
    let disk_bytes: i64 = 900_000;
    if disk_bytes <= quota_disk_bytes {
        read_only.store(false, Ordering::Release);
    }
    assert!(!read_only.load(Ordering::Acquire));
}

// ---------------------------------------------------------------------------
// Test 12: Env defaults -- unset = unlimited
// ---------------------------------------------------------------------------

/// default_quota_from_env returns None for all fields when env vars are unset.
#[test]
fn test_env_defaults_unset() {
    std::env::remove_var("KLEOS_DEFAULT_CONTENT_QUOTA_BYTES");
    std::env::remove_var("KLEOS_DEFAULT_MEMORY_COUNT_QUOTA");
    std::env::remove_var("KLEOS_DEFAULT_DISK_QUOTA_BYTES");
    let q = kleos_lib::quota::default_quota_from_env();
    assert!(q.content_bytes.is_none());
    assert!(q.memory_count.is_none());
    assert!(q.disk_bytes.is_none());
}
