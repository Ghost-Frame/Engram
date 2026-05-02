//! Core types for tenant management.

use crate::db::Database;
use crate::vector::VectorIndex;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Status of a tenant in the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantStatus {
    /// Tenant is active and can accept requests.
    Active,
    /// Tenant is suspended (quota exceeded, admin action, etc).
    Suspended,
    /// Tenant is being deleted.
    Deleting,
}

impl TenantStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TenantStatus::Active => "active",
            TenantStatus::Suspended => "suspended",
            TenantStatus::Deleting => "deleting",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(TenantStatus::Active),
            "suspended" => Some(TenantStatus::Suspended),
            "deleting" => Some(TenantStatus::Deleting),
            _ => None,
        }
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

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            max_resident: 512,
            idle_timeout: Duration::from_secs(15 * 60),
            preload_on_start: false,
        }
    }
}

/// A loaded tenant handle with database and vector index connections.
///
/// This struct represents a "live" tenant with open connections.
/// It is lazily loaded by the registry and evicted when idle.
pub struct TenantHandle {
    /// The computed tenant ID (safe for filesystem paths).
    pub tenant_id: String,

    /// The original user ID that maps to this tenant.
    pub user_id: String,

    /// The per-tenant async SQLite database (deadpool-sqlite pool).
    pub db: Arc<Database>,

    /// The per-tenant vector index (LanceDB).
    pub vector_index: Arc<dyn VectorIndex>,

    /// When this tenant was created.
    pub created_at: SystemTime,

    /// Last time this handle was accessed (for LRU eviction).
    pub last_access: Mutex<Instant>,
}

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

/// Configuration for tenant database connection pools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantPoolConfig {
    /// Maximum number of reader connections per tenant.
    pub max_readers: usize,
    /// Number of writer connections (usually 1 for SQLite).
    pub writer_count: usize,
    /// Busy timeout in milliseconds.
    pub busy_timeout_ms: u64,
    /// WAL autocheckpoint interval.
    pub wal_autocheckpoint: u64,
}

impl Default for TenantPoolConfig {
    fn default() -> Self {
        Self {
            max_readers: 4,
            writer_count: 1,
            busy_timeout_ms: 5_000,
            wal_autocheckpoint: 10_000,
        }
    }
}
