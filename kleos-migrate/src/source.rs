use anyhow::{anyhow, Result};
use rusqlite::Connection;
use std::path::Path;

pub struct SourceDb {
    pub conn: Connection,
}

/// Tables and patterns to skip during migration
pub const SKIP_TABLES: &[&str] = &[
    "rate_limits",
    "schema_version",
    "schema_versions",
    "vector_sync_pending",
    "app_state",
    "sqlite_sequence",
    // FTS virtual tables (rebuilt via TENANT_MIGRATIONS triggers)
    "memories_fts",
    "memories_fts_data",
    "memories_fts_idx",
    "memories_fts_docsize",
    "memories_fts_content",
    "memories_fts_config",
    "episodes_fts",
    "episodes_fts_data",
    "episodes_fts_idx",
    "episodes_fts_docsize",
    "episodes_fts_content",
    "episodes_fts_config",
    "messages_fts",
    "messages_fts_data",
    "messages_fts_idx",
    "messages_fts_docsize",
    "messages_fts_content",
    "messages_fts_config",
    "skills_fts",
    "skills_fts_data",
    "skills_fts_idx",
    "skills_fts_docsize",
    "skills_fts_content",
    "skills_fts_config",
    "artifacts_fts",
    "artifacts_fts_data",
    "artifacts_fts_idx",
    "artifacts_fts_docsize",
    "artifacts_fts_content",
    "artifacts_fts_config",
    // libsql vector index internals
    "libsql_vector_meta_shadow",
];

/// Open source SQLCipher (or plaintext) database.
pub fn open(path: &Path, key_env: Option<&str>) -> Result<SourceDb> {
    let conn = Connection::open(path)?;

    // Apply SQLCipher key if env var is set and non-empty.
    if let Some(env_name) = key_env {
        if let Ok(hex_key) = std::env::var(env_name) {
            if !hex_key.is_empty() {
                conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex_key))?;
                // SQLCipher 4 compatibility hint (covers older DBs created with cipher_compatibility=3)
                conn.execute_batch("PRAGMA cipher_compatibility = 4;")?;
            }
        }
    }

    // Verify we can actually read the schema. Catches bad key / corruption early.
    conn.query_row("PRAGMA schema_version", [], |_| Ok(()))
        .map_err(|_| anyhow!("source DB open failed: wrong key or not a database?"))?;

    Ok(SourceDb { conn })
}

/// Return all non-system, non-FTS, non-shadow table names from source.
pub fn get_tables(db: &SourceDb) -> Result<Vec<String>> {
    let mut stmt = db.conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    )?;
    let mut rows = stmt.query([])?;
    let mut tables = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        if !should_skip(&name) {
            tables.push(name);
        }
    }
    Ok(tables)
}

/// Return column names for the given table.
pub fn get_columns(db: &SourceDb, table: &str) -> Result<Vec<String>> {
    let mut stmt = db
        .conn
        .prepare(&format!("PRAGMA table_info(\"{}\")", table))?;
    let mut rows = stmt.query([])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        columns.push(name);
    }
    Ok(columns)
}

pub fn should_skip(table: &str) -> bool {
    SKIP_TABLES.contains(&table)
        || table.contains("_shadow")
        || table.contains("_fts_")
        || table.ends_with("_fts")
}
