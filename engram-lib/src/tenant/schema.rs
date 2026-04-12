//! Per-tenant database schema.
//!
//! This module defines the schema for per-tenant SQLite databases.
//! Key differences from the monolithic schema:
//! - No user_id columns (each database is for a single tenant)
//! - No user_id indexes
//! - No cross-tenant tables (those go in system/registry.db)

use rusqlite::Connection;

/// Current schema version for per-tenant databases.
pub const SCHEMA_VERSION: i64 = 1;

/// Create all tables in a per-tenant database.
pub fn create_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(include_str!("schema_v1.sql"))
}

/// Get the schema version from a database.
pub fn get_schema_version(conn: &Connection) -> Result<Option<i64>, rusqlite::Error> {
    let mut stmt =
        conn.prepare("SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 1")?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        Ok(Some(row.get(0)?))
    } else {
        Ok(None)
    }
}

/// Check if the schema needs migration.
pub fn needs_migration(conn: &Connection) -> Result<bool, rusqlite::Error> {
    match get_schema_version(conn)? {
        Some(v) => Ok(v < SCHEMA_VERSION),
        None => Ok(true),
    }
}
