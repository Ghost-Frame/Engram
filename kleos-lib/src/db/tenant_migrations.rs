//! Tenant-database migration chain.
//!
//! Each tenant shard has its own independent migration version tracked in the
//! `schema_migrations` table inside the tenant's own SQLite file. The version
//! sequence here is NOT related to the system/main migration sequence in
//! `super::migrations` -- system and tenant schemas evolve on separate
//! timelines.
//!
//! Migrations run lazily on tenant load (via `Database::open_tenant`). A new
//! tenant gets v1 applied; an existing tenant at v1 gets nothing until a new
//! version is appended to `TENANT_MIGRATIONS`.

use crate::{EngError, Result};
use rusqlite::Connection;
use tracing::info;

/// A single tenant-schema migration.
pub struct TenantMigration {
    pub version: i64,
    pub description: &'static str,
    pub up: fn(&Connection) -> Result<()>,
}

/// The canonical ordered list of tenant migrations.
///
/// Append-only. Never renumber, never edit a past entry.
pub static TENANT_MIGRATIONS: &[TenantMigration] = &[TenantMigration {
    version: 1,
    description: "initial_tenant_schema",
    up: apply_schema_v1,
}];

fn apply_schema_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(include_str!("../tenant/schema_v1.sql"))
        .map_err(|e| EngError::DatabaseMessage(format!("tenant schema v1 failed: {e}")))
}

/// Run all pending tenant migrations against `conn`.
///
/// Idempotent: safe to call on every tenant load. A freshly created tenant
/// database lands at the latest version; an existing one catches up.
pub fn run_tenant_migrations(conn: &Connection) -> Result<()> {
    // Tenant schema uses the `schema_migrations` table (as defined in v1).
    // Ensure it exists so we can read current_version even before v1 runs.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    let current: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

    for m in TENANT_MIGRATIONS.iter() {
        if m.version <= current {
            continue;
        }
        info!(
            "applying tenant migration {} ({})",
            m.version, m.description
        );
        (m.up)(conn)?;
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![m.version],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
    }

    Ok(())
}

/// Latest declared tenant schema version.
pub fn latest_version() -> i64 {
    TENANT_MIGRATIONS
        .iter()
        .map(|m| m.version)
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_db_lands_at_latest() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let v: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(v, latest_version());
    }

    #[test]
    fn idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();
        run_tenant_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn memories_table_exists_after_v1() {
        let conn = Connection::open_in_memory().unwrap();
        run_tenant_migrations(&conn).unwrap();

        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='memories'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }
}
