//! Core types for tenant management.

use crate::db::Database;
use crate::vector::VectorIndex;
use arc_swap::ArcSwap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Status of a tenant in the registry.
///
/// State machine: Active | Suspended -> Deleting -> Tombstone | Stuck.
/// Only `Active` tenants can serve requests (see `is_accessible`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantStatus {
    /// Tenant is active and can accept requests.
    Active,
    /// Tenant is suspended (quota exceeded, admin action, etc).
    Suspended,
    /// Tenant is being deleted (teardown in progress).
    Deleting,
    /// Teardown completed; username held for re-provision guard.
    Tombstone,
    /// Teardown failed after max attempts; needs manual intervention.
    Stuck,
}

/// String round-trip helpers for persisting the status in the registry.
impl TenantStatus {
    /// String representation used for storage and API serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantStatus::Active => "active",
            TenantStatus::Suspended => "suspended",
            TenantStatus::Deleting => "deleting",
            TenantStatus::Tombstone => "tombstone",
            TenantStatus::Stuck => "stuck",
        }
    }

    /// Parse a status string back into the enum.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(TenantStatus::Active),
            "suspended" => Some(TenantStatus::Suspended),
            "deleting" => Some(TenantStatus::Deleting),
            "tombstone" => Some(TenantStatus::Tombstone),
            "stuck" => Some(TenantStatus::Stuck),
            _ => None,
        }
    }

    /// Returns true only when the tenant can serve requests.
    pub fn is_accessible(self) -> bool {
        matches!(self, TenantStatus::Active)
    }
}

#[cfg(test)]
/// Tests for tenant type round-trips.
mod tests {
    use super::*;

    // Every status serializes and parses back to itself.
    #[test]
    fn tenant_status_round_trip() {
        for status in [
            TenantStatus::Active,
            TenantStatus::Suspended,
            TenantStatus::Deleting,
            TenantStatus::Tombstone,
            TenantStatus::Stuck,
        ] {
            let s = status.as_str();
            assert_eq!(
                TenantStatus::parse(s),
                Some(status),
                "round-trip failed for {s}"
            );
        }
        assert_eq!(TenantStatus::parse("garbage"), None);
        assert!(TenantStatus::Active.is_accessible());
        assert!(!TenantStatus::Suspended.is_accessible());
        assert!(!TenantStatus::Deleting.is_accessible());
        assert!(!TenantStatus::Tombstone.is_accessible());
        assert!(!TenantStatus::Stuck.is_accessible());
    }
}

/// Configuration for the tenant registry.
#[derive(Debug, Clone)]
pub struct TenantConfig {
    /// Maximum number of tenant handles to keep resident in memory.
    /// Default: 512
    pub max_resident: usize,

    /// How long an idle tenant handle stays resident before eviction.
    /// Default: 15 minutes
    pub idle_timeout: Duration,

    /// Whether to preload all tenants at startup.
    /// Default: false (lazy loading is preferred)
    pub preload_on_start: bool,
}

// Defaults sized for a small single-node deployment.
impl Default for TenantConfig {
    // 512 resident tenants, 15-minute idle eviction, lazy loading.
    fn default() -> Self {
        Self {
            max_resident: 512,
            idle_timeout: Duration::from_secs(15 * 60),
            preload_on_start: false,
        }
    }
}

/// Per-tenant quota limits loaded from the registry and cached on the handle.
///
/// All fields are `Option<i64>`: `None` means unlimited (backward-compatible
/// default for tenants created before E2). Set a value to enforce the limit.
#[derive(Debug, Clone)]
pub struct QuotaConfig {
    /// Maximum total size of stored memory content in bytes (hard quota).
    /// Enforced atomically inside the write transaction via enforce_quota_in_tx.
    pub content_bytes: Option<i64>,

    /// Maximum number of memory rows (hard quota).
    /// Enforced atomically inside the write transaction via enforce_quota_in_tx.
    pub memory_count: Option<i64>,

    /// Maximum disk usage of the entire tenant shard directory in bytes (soft quota).
    /// Enforced eventually by the background disk sampler; flips read_only on the handle.
    pub disk_bytes: Option<i64>,
}

// Default: unlimited quotas (enforced values come from the registry).
impl Default for QuotaConfig {
    /// Returns an unlimited quota (all fields None). Used for tenants with no
    /// configured limits; unlimited is the backward-compatible default.
    fn default() -> Self {
        Self {
            content_bytes: None,
            memory_count: None,
            disk_bytes: None,
        }
    }
}

/// A loaded tenant handle with database and vector index connections.
///
/// This struct represents a "live" tenant with open connections.
/// It is lazily loaded by the registry and evicted when idle.
/// Quota state is cached here for wait-free hot-path reads.
pub struct TenantHandle {
    /// The computed tenant ID (safe for filesystem paths).
    pub tenant_id: String,

    /// The original user ID that maps to this tenant.
    pub user_id: String,

    /// The per-tenant async SQLite database (deadpool-sqlite pool).
    pub db: Arc<Database>,

    /// The per-tenant ANN vector index (LanceDB). None when the crate is
    /// built without the `ml` feature, matching `Database::vector_index`;
    /// retrieval degrades to FTS + sqlite-vec.
    pub vector_index: Option<Arc<dyn VectorIndex>>,

    /// When this tenant was created.
    pub created_at: SystemTime,

    /// Last time this handle was accessed (for LRU eviction).
    pub last_access: Mutex<Instant>,

    /// Cached quota limits for this tenant. Updated by admin operations.
    /// ArcSwap enables wait-free reads on the write hot path without
    /// blocking the background quota-sync writer.
    pub quota: ArcSwap<QuotaConfig>,

    /// True if a counter-mutating write occurred since the last registry sync.
    /// The quota-sync job checks this flag with Relaxed ordering (advisory).
    pub dirty: AtomicBool,

    /// True when the disk quota is exceeded. Set by the disk sampler job.
    /// Uses Acquire/Release ordering because the write path reads this flag
    /// to short-circuit before entering a transaction.
    pub read_only: AtomicBool,

    /// Path to the tenant shard directory. Used by the disk sampler for du.
    pub shard_path: PathBuf,
}

/// Access-tracking and lifecycle helpers for a resident tenant.
impl TenantHandle {
    /// Update the last access time to now.
    pub fn touch(&self) {
        if let Ok(mut last) = self.last_access.lock() {
            *last = Instant::now();
        }
    }

    /// Get the time since last access.
    pub fn idle_duration(&self) -> Duration {
        self.last_access
            .lock()
            .map(|last| last.elapsed())
            .unwrap_or(Duration::ZERO)
    }

    /// Get the async Database for this tenant.
    ///
    /// Returns a clone of the Arc already held in the handle. Kept as a
    /// method (rather than requiring callers to reach into `.db`) to leave
    /// room for per-request tracking, quota checks, or rate limiting later.
    pub fn database(&self) -> Arc<Database> {
        Arc::clone(&self.db)
    }

    /// Load the current quota configuration. Wait-free via ArcSwap.
    pub fn quota(&self) -> Arc<QuotaConfig> {
        self.quota.load_full()
    }

    /// Replace the cached quota configuration. Called by admin update routes.
    pub fn refresh_quota(&self, new: QuotaConfig) {
        self.quota.store(Arc::new(new));
    }

    /// Mark this handle as having unsynced counter mutations.
    /// Uses Relaxed ordering -- this is an advisory dirty flag, not a lock.
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Atomically take the dirty flag. Returns true if the handle was dirty.
    /// Used by the quota-sync job to decide which handles need a registry flush.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::Relaxed)
    }

    /// Returns true when the disk quota is exceeded and writes are blocked.
    /// Uses Acquire ordering so the disk sampler's Release store is visible.
    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::Acquire)
    }

    /// Set or clear the read-only flag. Called by the disk sampler.
    /// Uses Release ordering so subsequent Acquire loads see the update.
    pub fn set_read_only(&self, val: bool) {
        self.read_only.store(val, Ordering::Release);
    }

    /// Return the path to the tenant shard directory. Used by du_bytes.
    pub fn shard_path(&self) -> &Path {
        &self.shard_path
    }
}

/// A row from the tenant registry database (system/registry.db).
#[derive(Debug, Clone)]
pub struct TenantRow {
    /// The computed tenant ID (safe for filesystem paths).
    pub tenant_id: String,

    /// The original user ID.
    pub user_id: String,

    /// When this tenant was created (unix timestamp).
    pub created_at: i64,

    /// Current status.
    pub status: TenantStatus,

    /// Path to the tenant's data directory.
    pub data_path: String,

    /// Schema version of the tenant's database.
    pub schema_version: i64,

    /// Optional quota on disk usage in bytes.
    pub quota_bytes: Option<i64>,

    /// Optional quota on number of memories.
    pub quota_memories: Option<i64>,

    /// Last access time (unix timestamp).
    pub last_access: i64,
}

/// Raw quota row returned from the registry for the admin quota endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TenantQuotaRow {
    /// The user_id for this tenant.
    pub user_id: String,
    /// Configured content bytes limit (None = unlimited).
    pub quota_content_bytes: Option<i64>,
    /// Configured memory count limit (None = unlimited).
    pub quota_memory_count: Option<i64>,
    /// Configured disk bytes limit (None = unlimited).
    pub quota_disk_bytes: Option<i64>,
    /// Last-synced content bytes usage.
    pub content_bytes_used: i64,
    /// Last-synced memory count usage.
    pub memory_count_used: i64,
    /// Last-synced disk bytes usage.
    pub disk_bytes_used: i64,
    /// When the usage was last synced (ISO 8601 datetime).
    pub last_synced_at: Option<String>,
}
