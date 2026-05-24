//! Regression test for E3: concurrent first-touch of the same tenant must open
//! exactly one shard (one `Database` writer pool + one `LanceIndex`), not one
//! per racing caller.
//!
//! Without single-flight in `TenantLoader::get_or_load`, N concurrent callers
//! all miss the resident fast-path and each run `load_tenant`, opening N pools
//! on one shard. That breaks the single-writer invariant (FP-2) during the
//! first-touch window. With single-flight, every concurrent caller receives the
//! same `Arc<TenantHandle>`.

use std::sync::Arc;
use tempfile::tempdir;

use kleos_lib::tenant::{TenantConfig, TenantRegistry};

/// Spawn many concurrent first-touch loads of one tenant and assert they all
/// resolve to the identical handle (proving a single open).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_touch_opens_single_handle() {
    let dir = tempdir().expect("tempdir");
    let registry = Arc::new(
        TenantRegistry::new(dir.path(), TenantConfig::default(), 128, false, None)
            .expect("registry"),
    );

    // Fire many concurrent first-touch loads for the SAME user.
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let reg = Arc::clone(&registry);
        tasks.push(tokio::spawn(async move {
            reg.get_or_create("racing_user").await.expect("load tenant")
        }));
    }

    let mut handles = Vec::new();
    for t in tasks {
        handles.push(t.await.expect("join task"));
    }

    // Single-flight invariant: every caller got the same handle Arc.
    let first = &handles[0];
    for (i, h) in handles.iter().enumerate().skip(1) {
        assert!(
            Arc::ptr_eq(first, h),
            "caller {i} received a different TenantHandle: concurrent first-touch \
             opened more than one shard pool (single-flight missing)"
        );
    }

    // Exactly one tenant resident.
    assert_eq!(registry.resident_count().await, 1);

    std::mem::forget(dir);
}
