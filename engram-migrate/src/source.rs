use anyhow::Result;
use libsql::{Builder, Connection};
use std::collections::HashMap;
use std::path::Path;

pub struct SourceDb {
    pub conn: Connection,
}

/// Open libsql database at the given path
pub async fn open(path: &Path) -> Result<SourceDb> {
    let db = Builder::new_local(path.to_string_lossy().as_ref())
        .build()
        .await?;
    let conn = db.connect()?;
    Ok(SourceDb { conn })
}

/// Analyze source database, returning table names and row counts
/// Uses MAX(rowid) as proxy for COUNT(*) due to libsql vector extension bug
pub async fn analyze(db: &SourceDb) -> Result<HashMap<String, i64>> {
    let mut tables = HashMap::new();

    // Get all table names
    let mut stmt = db.conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"
    ).await?;

    let mut rows = stmt.query(()).await?;
    let mut table_names = Vec::new();

    while let Some(row) = rows.next().await? {
        let name: String = row.get(0)?;
        table_names.push(name);
    }

    // Get row count for each table
    for name in table_names {
        // Skip internal libsql tables and FTS tables
        if name.contains("_shadow")
            || name.contains("_idx")
            || name.ends_with("_fts")
            || name.contains("_fts_")
            || name == "libsql_vector_meta_shadow"
        {
            continue;
        }

        // Try MAX(rowid) first, fall back to COUNT(*) if rowid doesn't exist
        let count = match try_max_rowid(db, &name).await {
            Ok(c) => c,
            Err(_) => {
                // Table might be WITHOUT ROWID or a virtual table, try COUNT(*)
                match try_count(db, &name).await {
                    Ok(c) => c,
                    Err(_) => 0, // Skip tables we can't count
                }
            }
        };

        tables.insert(name, count);
    }

    Ok(tables)
}

async fn try_max_rowid(db: &SourceDb, table: &str) -> Result<i64> {
    let query = format!("SELECT MAX(rowid) FROM \"{}\"", table);
    let mut stmt = db.conn.prepare(&query).await?;
    let mut rows = stmt.query(()).await?;
    if let Some(row) = rows.next().await? {
        Ok(row.get::<i64>(0).unwrap_or(0))
    } else {
        Ok(0)
    }
}

async fn try_count(db: &SourceDb, table: &str) -> Result<i64> {
    let query = format!("SELECT COUNT(*) FROM \"{}\"", table);
    let mut stmt = db.conn.prepare(&query).await?;
    let mut rows = stmt.query(()).await?;
    if let Some(row) = rows.next().await? {
        Ok(row.get::<i64>(0).unwrap_or(0))
    } else {
        Ok(0)
    }
}

/// Get column names for a table
pub async fn get_columns(db: &SourceDb, table: &str) -> Result<Vec<String>> {
    let query = format!("PRAGMA table_info(\"{}\")", table);
    let mut stmt = db.conn.prepare(&query).await?;
    let mut rows = stmt.query(()).await?;

    let mut columns = Vec::new();
    while let Some(row) = rows.next().await? {
        let name: String = row.get(1)?; // column 1 is 'name'
        columns.push(name);
    }

    Ok(columns)
}

/// Get all table names (excluding internal tables)
pub async fn get_tables(db: &SourceDb) -> Result<Vec<String>> {
    let mut stmt = db.conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    ).await?;

    let mut rows = stmt.query(()).await?;
    let mut tables = Vec::new();

    while let Some(row) = rows.next().await? {
        let name: String = row.get(0)?;

        // Skip internal libsql tables
        if name.contains("_shadow") || name.contains("_idx") || name == "libsql_vector_meta_shadow" {
            continue;
        }

        tables.push(name);
    }

    Ok(tables)
}
