//! Per-tenant database sharding module.
//!
//! This module implements physical tenant isolation where each tenant gets:
//! - Their own SQLite database file at `data_dir/tenants/<tenant_id>/engram.db`
//! - Their own LanceDB HNSW index at `data_dir/tenants/<tenant_id>/hnsw/memories.lance`
//!
//! The `TenantRegistry` manages tenant lifecycle, lazy loading, and LRU eviction.

pub mod id;
pub mod loader;
pub mod pool;
pub mod ratelimit;
pub mod registry;
pub mod registry_db;
pub mod schema;
pub mod types;

pub use id::tenant_id_from_user;
pub use pool::TenantPools;
pub use registry::TenantRegistry;
pub use types::{TenantConfig, TenantHandle, TenantPoolConfig, TenantRow, TenantStatus};

/// Reserved tenant id that owns the cross-user session-handoff table set
/// (schema_v43). The string is ASCII-safe so `tenant_id_from_user` returns
/// it unchanged; the on-disk shard lives at `data_dir/tenants/handoffs/`.
pub const HANDOFFS_TENANT_ID: &str = "handoffs";

/// Tenant submodules prefer `EngError::Internal` with a contextual message
/// (see `tenant/registry_db.rs`), so this generic `DatabaseMessage` converter
/// is not used on their hot paths. Kept as the default conversion for future
/// tenant-level read paths that do not need custom context.
#[allow(dead_code)]
fn rusqlite_to_eng_error(err: rusqlite::Error) -> crate::EngError {
    crate::EngError::DatabaseMessage(err.to_string())
}
