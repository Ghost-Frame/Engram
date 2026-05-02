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
///
/// If the env var named by `key_env` is set and non-empty, the source is
/// opened as SQLCipher. The tool tries compat 4 first (current default),
/// then falls back to compat 3 for older Engram databases. compat PRAGMAs
/// must be set BEFORE the key or they are no-ops.
pub fn open(path: &Path, key_env: Option<&str>) -> Result<SourceDb> {
    let hex_key = key_env
        .and_then(|name| std::env::var(name).ok())
        .filter(|k| !k.is_empty());

    if let Some(key) = hex_key {
        // Try modern SQLCipher 4 first (bundled-sqlcipher default).
        if let Ok(db) = try_open_encrypted(path, &key, 4) {
            return Ok(db);
        }
        // Fall back to compat 3 for DBs created by older SQLCipher builds.
        return try_open_encrypted(path, &key, 3).map_err(|e| {
            anyhow!(
                "source DB open failed with both cipher_compatibility=4 and =3: {e}. \
                 Wrong key, not a database, or unsupported SQLCipher version?"
            )
        });
    }

    // Plaintext path.
    let conn = Connection::open(path)?;
    verify_readable(&conn)?;
    Ok(SourceDb { conn })
}

fn try_open_encrypted(path: &Path, hex_key: &str, compat: u8) -> Result<SourceDb> {
    let conn = Connection::open(path)?;
    // Compat PRAGMA MUST precede the key pragma. SQLCipher ignores it once
    // the key has been applied.
    conn.execute_batch(&format!("PRAGMA cipher_compatibility = {compat};"))?;
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex_key))?;
    verify_readable(&conn)?;
    Ok(SourceDb { conn })
}

/// Force a page-level decrypt so a wrong key surfaces as an error.
///
/// `PRAGMA schema_version` reads from a header cache on some SQLCipher
/// builds and can pass with a wrong key. `SELECT count(*) FROM sqlite_master`
/// must decrypt at least one data page so it is the correct probe.
fn verify_readable(conn: &Connection) -> Result<()> {
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|_| ())
    .map_err(|_| anyhow!("source DB open failed: wrong key or not a database?"))
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
