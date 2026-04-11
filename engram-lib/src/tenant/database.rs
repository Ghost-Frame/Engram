//! Per-tenant database wrapper using rusqlite.
//!
//! Each tenant gets their own SQLite database file. This module provides
//! the connection wrapper and schema management.

use super::schema;
use crate::EngError;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::info;

/// A per-tenant SQLite database using rusqlite.
///
/// This is distinct from the monolithic `Database` type which uses libsql.
/// Each tenant has their own file at `data_dir/tenants/<tenant_id>/engram.db`.
pub struct TenantDatabase {
    /// The underlying rusqlite connection.
    /// Wrapped in Mutex for thread-safety since rusqlite Connection is !Sync.
    conn: Mutex<Connection>,

    /// Path to the database file.
    path: PathBuf,
}

impl TenantDatabase {
    /// Open or create a tenant database at the given path.
    ///
    /// Creates the schema if this is a new database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EngError> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                EngError::Internal(format!("failed to create tenant directory: {}", e))
            })?;
        }

        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| EngError::Internal(format!("failed to open tenant database: {}", e)))?;

        // Configure pragmas
        Self::configure_pragmas(&conn)?;

        // Create schema if needed
        if schema::needs_migration(&conn).map_err(|e| {
            EngError::Internal(format!("failed to check schema version: {}", e))
        })? {
            schema::create_tables(&conn).map_err(|e| {
                EngError::Internal(format!("failed to create tenant schema: {}", e))
            })?;
            info!("tenant database schema created: {}", path.display());
        }

        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    /// Open an in-memory database for testing.
    pub fn open_memory() -> Result<Self, EngError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| EngError::Internal(format!("failed to open in-memory database: {}", e)))?;

        // Foreign keys only for in-memory
        conn.execute("PRAGMA foreign_keys = ON", [])
            .map_err(|e| EngError::Internal(format!("failed to set pragma: {}", e)))?;

        schema::create_tables(&conn)
            .map_err(|e| EngError::Internal(format!("failed to create schema: {}", e)))?;

        Ok(Self {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        })
    }

    /// Configure SQLite pragmas for optimal performance.
    fn configure_pragmas(conn: &Connection) -> Result<(), EngError> {
        let pragmas = [
            "PRAGMA journal_mode = WAL",
            "PRAGMA synchronous = NORMAL",
            "PRAGMA cache_size = -16000", // 16MB cache per tenant
            "PRAGMA foreign_keys = ON",
            "PRAGMA busy_timeout = 5000",
            "PRAGMA temp_store = MEMORY",
            "PRAGMA mmap_size = 67108864", // 64MB mmap per tenant
        ];

        for pragma in pragmas {
            conn.execute(pragma, [])
                .map_err(|e| EngError::Internal(format!("failed to set pragma: {}", e)))?;
        }

        Ok(())
    }

    /// Get the database path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Execute a query with no return value.
    pub fn execute(&self, sql: &str, params: &[&dyn rusqlite::ToSql]) -> Result<usize, EngError> {
        let conn = self.conn.lock().map_err(|_| {
            EngError::Internal("failed to acquire database lock".to_string())
        })?;
        conn.execute(sql, params)
            .map_err(|e| EngError::Internal(format!("query failed: {}", e)))
    }

    /// Execute a query and map results.
    pub fn query_map<T, F>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        map_fn: F,
    ) -> Result<Vec<T>, EngError>
    where
        F: FnMut(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let conn = self.conn.lock().map_err(|_| {
            EngError::Internal("failed to acquire database lock".to_string())
        })?;
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| EngError::Internal(format!("failed to prepare query: {}", e)))?;
        let rows = stmt
            .query_map(params, map_fn)
            .map_err(|e| EngError::Internal(format!("query failed: {}", e)))?;

        rows.collect::<Result<Vec<T>, _>>()
            .map_err(|e| EngError::Internal(format!("failed to map row: {}", e)))
    }

    /// Execute a query and return a single optional result.
    pub fn query_one<T, F>(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
        map_fn: F,
    ) -> Result<Option<T>, EngError>
    where
        F: FnOnce(&rusqlite::Row<'_>) -> Result<T, rusqlite::Error>,
    {
        let conn = self.conn.lock().map_err(|_| {
            EngError::Internal("failed to acquire database lock".to_string())
        })?;
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| EngError::Internal(format!("failed to prepare query: {}", e)))?;
        let mut rows = stmt
            .query(params)
            .map_err(|e| EngError::Internal(format!("query failed: {}", e)))?;

        match rows.next() {
            Ok(Some(row)) => map_fn(row)
                .map(Some)
                .map_err(|e| EngError::Internal(format!("failed to map row: {}", e))),
            Ok(None) => Ok(None),
            Err(e) => Err(EngError::Internal(format!("query failed: {}", e))),
        }
    }

    /// Run a transaction.
    pub fn transaction<T, F>(&self, f: F) -> Result<T, EngError>
    where
        F: FnOnce(&Connection) -> Result<T, EngError>,
    {
        let mut conn = self.conn.lock().map_err(|_| {
            EngError::Internal("failed to acquire database lock".to_string())
        })?;
        let tx = conn
            .transaction()
            .map_err(|e| EngError::Internal(format!("failed to start transaction: {}", e)))?;

        let result = f(&tx)?;

        tx.commit()
            .map_err(|e| EngError::Internal(format!("failed to commit transaction: {}", e)))?;

        Ok(result)
    }

    /// Checkpoint the WAL and close cleanly.
    /// Call this before eviction to ensure data is flushed.
    pub fn checkpoint(&self) -> Result<(), EngError> {
        let conn = self.conn.lock().map_err(|_| {
            EngError::Internal("failed to acquire database lock".to_string())
        })?;
        conn.execute("PRAGMA wal_checkpoint(TRUNCATE)", [])
            .map_err(|e| EngError::Internal(format!("checkpoint failed: {}", e)))?;
        Ok(())
    }
}

impl std::fmt::Debug for TenantDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TenantDatabase")
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_memory() {
        let db = TenantDatabase::open_memory().unwrap();
        assert_eq!(db.path().to_str(), Some(":memory:"));
    }

    #[test]
    fn test_execute_and_query() {
        let db = TenantDatabase::open_memory().unwrap();

        // Insert a memory
        db.execute(
            "INSERT INTO memories (content, category) VALUES (?1, ?2)",
            &[&"test content", &"test"],
        )
        .unwrap();

        // Query it back
        let content: Option<String> = db
            .query_one("SELECT content FROM memories WHERE id = 1", &[], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(content, Some("test content".to_string()));
    }

    #[test]
    fn test_transaction() {
        let db = TenantDatabase::open_memory().unwrap();

        let result = db.transaction(|tx| {
            tx.execute(
                "INSERT INTO memories (content, category) VALUES (?1, ?2)",
                ["tx content", "test"],
            )
            .map_err(|e| EngError::Internal(e.to_string()))?;
            Ok("done")
        });

        assert!(result.is_ok());

        let count: Option<i64> = db
            .query_one("SELECT COUNT(*) FROM memories", &[], |row| row.get(0))
            .unwrap();
        assert_eq!(count, Some(1));
    }
}
