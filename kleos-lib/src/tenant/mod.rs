//! Per-tenant database sharding module.
//!
//! This module implements physical tenant isolation where each tenant gets:
//! - Their own SQLite database file at `data_dir/tenants/<tenant_id>/engram.db`
//! - Their own LanceDB HNSW index at `data_dir/tenants/<tenant_id>/hnsw/memories.lance`
//!
//! The `TenantRegistry` manages tenant lifecycle, lazy loading, and LRU eviction.

pub mod id;
pub mod loader;
pub mod registry;
pub mod registry_db;
pub mod teardown;
pub mod types;

pub use id::tenant_id_from_user;
pub use registry::TenantRegistry;
pub use teardown::{
    DeprovisionId, DeprovisionReport, RecoveryReport, TeardownStatus, TeardownStep,
};
pub use types::{TenantConfig, TenantHandle, TenantRow, TenantStatus};

/// Reserved tenant id that owns the cross-user session-handoff table set
/// (schema_v43). The string is ASCII-safe so `tenant_id_from_user` returns
/// it unchanged; the on-disk shard lives at `data_dir/tenants/handoffs/`.
pub const HANDOFFS_TENANT_ID: &str = "handoffs";

/// Reserved tenant id that owns the cross-machine Frameshift growth-log set
/// (schema_v73). Like handoffs, it is a single shared shard whose rows are
/// scoped by `user_id`, so all of one operator's machines (which authenticate
/// as the same user) converge on one logical growth set. The string is
/// ASCII-safe so `tenant_id_from_user` returns it unchanged; the on-disk shard
/// lives at `data_dir/tenants/frameshift-growth/`.
pub const FRAMESHIFT_GROWTH_TENANT_ID: &str = "frameshift-growth";
