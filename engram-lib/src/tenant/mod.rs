//! Per-tenant database sharding module.
//!
//! This module implements physical tenant isolation where each tenant gets:
//! - Their own SQLite database file at `data_dir/tenants/<tenant_id>/engram.db`
//! - Their own LanceDB HNSW index at `data_dir/tenants/<tenant_id>/hnsw/memories.lance`
//!
//! The `TenantRegistry` manages tenant lifecycle, lazy loading, and LRU eviction.

pub mod database;
pub mod id;
pub mod loader;
pub mod registry;
pub mod registry_db;
pub mod schema;
pub mod types;

pub use database::TenantDatabase;
pub use id::tenant_id_from_user;
pub use registry::TenantRegistry;
pub use types::{TenantConfig, TenantHandle, TenantRow, TenantStatus};
