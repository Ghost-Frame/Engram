use anyhow::{anyhow, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub struct TargetDb {
    pub db: kleos_lib::db::Database,
    pub target_dir: PathBuf,
    pub kleos_db_path: PathBuf,
}

/// Open (or create) the tenant shard at `target_dir/kleos.db`.
/// Runs TENANT_MIGRATIONS automatically via `Database::open_tenant`.
pub async fn open(target_dir: &Path) -> Result<TargetDb> {
    std::fs::create_dir_all(target_dir)?;
    let kleos_db_path = target_dir.join("kleos.db");

    let db = kleos_lib::db::Database::open_tenant(
        kleos_db_path
            .to_str()
            .ok_or_else(|| anyhow!("target path is not valid UTF-8"))?,
        None,
    )
    .await
    .map_err(|e| anyhow!("open tenant db: {e}"))?;

    Ok(TargetDb {
        db,
        target_dir: target_dir.to_path_buf(),
        kleos_db_path,
    })
}

/// Open a fresh unpooled read/write connection on the target DB for bulk inserts.
///
/// The pooled `Database` is only used for schema-init during this tool's run;
/// a direct rusqlite connection on the same file is fine because the server
/// isn't using it.
pub fn raw_conn(target: &TargetDb) -> Result<Connection> {
    let conn = Connection::open(&target.kleos_db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; \
         PRAGMA busy_timeout=30000;",
    )?;
    Ok(conn)
}

/// Return column names for `table` in the target schema.
pub fn get_target_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{}\")", table))?;
    let mut rows = stmt.query([])?;
    let mut columns = Vec::new();
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        columns.push(name);
    }
    Ok(columns)
}

/// Return true if `table` exists in the target schema.
pub fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1 LIMIT 1",
            rusqlite::params![table],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(count == 1)
}
