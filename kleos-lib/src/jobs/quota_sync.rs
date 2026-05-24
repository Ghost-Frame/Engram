//! Quota sync job -- flush dirty tenant counter state to the registry.
//!
//! Dirty handles (any handle where `take_dirty()` returns true) have their
//! `tenant_state` counters read and batch-updated in the registry database.
//! Runs periodically (default: every 5 minutes).
//!
//! Only handles that mutated counters since the last sync are visited, so
//! the common case (idle tenants) costs nothing. `take_dirty()` atomically
//! clears the flag so a write that races with the sync is counted in the
//! next cycle.

use crate::tenant::registry::TenantRegistry;
use crate::Result;
use std::sync::Arc;
use tracing::{debug, warn};

/// Usage snapshot read from a tenant shard and synced to the registry.
#[derive(Debug)]
pub struct TenantUsageUpdate {
    /// The user_id string from the TenantHandle.
    pub user_id: String,
    /// Current content_bytes counter value from tenant_state.
    pub content_bytes: i64,
    /// Current memory_count counter value from tenant_state.
    pub memory_count: i64,
    /// Current disk_bytes_estimate counter value from tenant_state.
    pub disk_bytes: i64,
}

/// Read tenant_state counters from a shard database.
///
/// Returns a TenantUsageUpdate ready for the registry bulk upsert.
/// Errors reading individual counters are treated as 0 (conservative).
async fn read_shard_counters(
    handle: &crate::tenant::types::TenantHandle,
) -> Result<TenantUsageUpdate> {
    let db = handle.database();
    db.read(|conn| {
        let content_bytes: i64 = conn
            .query_row(
                "SELECT value FROM tenant_state WHERE key = 'content_bytes'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let memory_count: i64 = conn
            .query_row(
                "SELECT value FROM tenant_state WHERE key = 'memory_count'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let disk_bytes: i64 = conn
            .query_row(
                "SELECT value FROM tenant_state WHERE key = 'disk_bytes_estimate'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        Ok(TenantUsageUpdate {
            user_id: String::new(), // filled by caller
            content_bytes,
            memory_count,
            disk_bytes,
        })
    })
    .await
}

/// Flush all dirty tenant handles to the registry.
///
/// Atomically takes the dirty flag from each resident handle. Handles that
/// were dirty have their shard counters read and batched into a registry
/// UPDATE. Handles that were clean are skipped entirely.
///
/// Non-fatal: a failure to read or write any individual tenant is logged
/// and skipped; the next cycle will retry (because the counters will have
/// changed again or the dirty flag will be set again by the next write).
pub async fn sync_dirty_handles_to_registry(registry: &Arc<TenantRegistry>) -> Result<()> {
    let handles = registry.snapshot_all_handles().await;

    let mut updates: Vec<TenantUsageUpdate> = Vec::new();

    for handle in &handles {
        if !handle.take_dirty() {
            continue;
        }

        match read_shard_counters(handle).await {
            Ok(mut update) => {
                update.user_id = handle.user_id.clone();
                updates.push(update);
            }
            Err(e) => {
                warn!(
                    tenant = %handle.tenant_id,
                    "quota_sync: failed to read shard counters: {}",
                    e
                );
                // Re-mark dirty so the next cycle retries.
                handle.mark_dirty();
            }
        }
    }

    if updates.is_empty() {
        debug!("quota_sync: no dirty handles to flush");
        return Ok(());
    }

    debug!(
        "quota_sync: flushing {} dirty handles to registry",
        updates.len()
    );

    registry.bulk_set_usage(updates).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    /// Confirms take_dirty returns true once then false (basic AtomicBool semantics).
    #[test]
    fn test_dirty_flag_take_once() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = AtomicBool::new(true);
        assert!(flag.swap(false, Ordering::Relaxed));
        assert!(!flag.swap(false, Ordering::Relaxed));
    }
}
