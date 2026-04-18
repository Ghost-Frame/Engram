//! Registry database for tenant metadata.
//!
//! The registry lives at `data_dir/system/registry.db` and tracks:
//! - All tenants and their metadata
//! - Quotas and status
//! - Schema versions

use super::types::{TenantRow, TenantStatus};
use crate::EngError;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::info;

/// Schema for the registry database.
const REGISTRY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS tenants (
    tenant_id TEXT PRIMARY KEY,
    user_id TEXT UNIQUE NOT NULL,
    created_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    data_path TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    quota_bytes INTEGER,
    quota_memories INTEGER,
    last_access INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tenants_user_id ON tenants(user_id);
CREATE INDEX IF NOT EXISTS idx_tenants_last_access ON tenants(last_access);
CREATE INDEX IF NOT EXISTS idx_tenants_status ON tenants(status);
"#;

/// Connection to the registry database.
pub struct RegistryDb {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl RegistryDb {
    /// Open or create the registry database.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, EngError> {
        let system_dir = data_dir.as_ref().join("system");
        std::fs::create_dir_all(&system_dir)
            .map_err(|e| EngError::Internal(format!("failed to create system directory: {}", e)))?;

        let path = system_dir.join("registry.db");
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|e| EngError::Internal(format!("failed to open registry database: {}", e)))?;

        // Configure pragmas
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(|e| EngError::Internal(format!("failed to set pragmas: {}", e)))?;

        // Create schema
        conn.execute_batch(REGISTRY_SCHEMA)
            .map_err(|e| EngError::Internal(format!("failed to create registry schema: {}", e)))?;

        info!("registry database opened: {}", path.display());

        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    /// Open an in-memory registry for testing.
    pub fn open_memory() -> Result<Self, EngError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| EngError::Internal(format!("failed to open in-memory registry: {}", e)))?;

        conn.execute_batch(REGISTRY_SCHEMA)
            .map_err(|e| EngError::Internal(format!("failed to create registry schema: {}", e)))?;

        Ok(Self {
            conn: Mutex::new(conn),
            path: PathBuf::from(":memory:"),
        })
    }

    /// Get a tenant by user_id.
    pub fn get_by_user_id(&self, user_id: &str) -> Result<Option<TenantRow>, EngError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT tenant_id, user_id, created_at, status, data_path,
                        schema_version, quota_bytes, quota_memories, last_access
                 FROM tenants WHERE user_id = ?1",
            )
            .map_err(|e| EngError::Internal(format!("failed to prepare query: {}", e)))?;

        let mut rows = stmt
            .query([user_id])
            .map_err(|e| EngError::Internal(format!("query failed: {}", e)))?;

        match rows.next() {
            Ok(Some(row)) => Ok(Some(Self::row_to_tenant(row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(EngError::Internal(format!("failed to fetch row: {}", e))),
        }
    }

    /// Get a tenant by tenant_id.
    pub fn get_by_tenant_id(&self, tenant_id: &str) -> Result<Option<TenantRow>, EngError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT tenant_id, user_id, created_at, status, data_path,
                        schema_version, quota_bytes, quota_memories, last_access
                 FROM tenants WHERE tenant_id = ?1",
            )
            .map_err(|e| EngError::Internal(format!("failed to prepare query: {}", e)))?;

        let mut rows = stmt
            .query([tenant_id])
            .map_err(|e| EngError::Internal(format!("query failed: {}", e)))?;

        match rows.next() {
            Ok(Some(row)) => Ok(Some(Self::row_to_tenant(row)?)),
            Ok(None) => Ok(None),
            Err(e) => Err(EngError::Internal(format!("failed to fetch row: {}", e))),
        }
    }

    /// Insert a new tenant.
    pub fn insert(&self, row: &TenantRow) -> Result<(), EngError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO tenants (tenant_id, user_id, created_at, status, data_path,
                                  schema_version, quota_bytes, quota_memories, last_access)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                row.tenant_id,
                row.user_id,
                row.created_at,
                row.status.as_str(),
                row.data_path,
                row.schema_version,
                row.quota_bytes,
                row.quota_memories,
                row.last_access,
            ],
        )
        .map_err(|e| EngError::Internal(format!("failed to insert tenant: {}", e)))?;
        Ok(())
    }

    /// Insert a new tenant, or return existing row if user_id already exists.
    ///
    /// This handles the TOCTOU race in get_or_create by using INSERT OR IGNORE
    /// and then fetching the existing row.
    pub fn insert_or_get(&self, row: &TenantRow) -> Result<TenantRow, EngError> {
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO tenants (tenant_id, user_id, created_at, status, data_path,
                                            schema_version, quota_bytes, quota_memories, last_access)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                row.tenant_id,
                row.user_id,
                row.created_at,
                row.status.as_str(),
                row.data_path,
                row.schema_version,
                row.quota_bytes,
                row.quota_memories,
                row.last_access,
            ],
        )
        .map_err(|e| EngError::Internal(format!("failed to insert tenant: {}", e)))?;

        // Fetch the row (either we inserted it or it already existed)
        drop(conn);
        self.get_by_user_id(&row.user_id)?
            .ok_or_else(|| EngError::Internal("tenant row disappeared after insert".to_string()))
    }

    /// Update tenant status.
    pub fn update_status(&self, tenant_id: &str, status: TenantStatus) -> Result<(), EngError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE tenants SET status = ?1 WHERE tenant_id = ?2",
            rusqlite::params![status.as_str(), tenant_id],
        )
        .map_err(|e| EngError::Internal(format!("failed to update tenant status: {}", e)))?;
        Ok(())
    }

    /// Update last access time.
    pub fn touch(&self, tenant_id: &str) -> Result<(), EngError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let conn = self.lock()?;
        conn.execute(
            "UPDATE tenants SET last_access = ?1 WHERE tenant_id = ?2",
            rusqlite::params![now, tenant_id],
        )
        .map_err(|e| EngError::Internal(format!("failed to update last access: {}", e)))?;
        Ok(())
    }

    /// Delete a tenant.
    pub fn delete(&self, tenant_id: &str) -> Result<(), EngError> {
        let conn = self.lock()?;
        conn.execute("DELETE FROM tenants WHERE tenant_id = ?1", [tenant_id])
            .map_err(|e| EngError::Internal(format!("failed to delete tenant: {}", e)))?;
        Ok(())
    }

    /// List all tenants.
    pub fn list(&self) -> Result<Vec<TenantRow>, EngError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT tenant_id, user_id, created_at, status, data_path,
                        schema_version, quota_bytes, quota_memories, last_access
                 FROM tenants ORDER BY created_at DESC",
            )
            .map_err(|e| EngError::Internal(format!("failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(TenantRow {
                    tenant_id: row.get(0)?,
                    user_id: row.get(1)?,
                    created_at: row.get(2)?,
                    status: TenantStatus::parse(row.get::<_, String>(3)?.as_str())
                        .unwrap_or(TenantStatus::Active),
                    data_path: row.get(4)?,
                    schema_version: row.get(5)?,
                    quota_bytes: row.get(6)?,
                    quota_memories: row.get(7)?,
                    last_access: row.get(8)?,
                })
            })
            .map_err(|e| EngError::Internal(format!("query failed: {}", e)))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| EngError::Internal(format!("failed to collect rows: {}", e)))
    }

    /// List tenants that are idle (last_access older than threshold).
    pub fn list_idle(&self, older_than_secs: i64) -> Result<Vec<TenantRow>, EngError> {
        let threshold = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64 - older_than_secs)
            .unwrap_or(0);

        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT tenant_id, user_id, created_at, status, data_path,
                        schema_version, quota_bytes, quota_memories, last_access
                 FROM tenants
                 WHERE last_access < ?1 AND status = 'active'
                 ORDER BY last_access ASC",
            )
            .map_err(|e| EngError::Internal(format!("failed to prepare query: {}", e)))?;

        let rows = stmt
            .query_map([threshold], |row| {
                Ok(TenantRow {
                    tenant_id: row.get(0)?,
                    user_id: row.get(1)?,
                    created_at: row.get(2)?,
                    status: TenantStatus::parse(row.get::<_, String>(3)?.as_str())
                        .unwrap_or(TenantStatus::Active),
                    data_path: row.get(4)?,
                    schema_version: row.get(5)?,
                    quota_bytes: row.get(6)?,
                    quota_memories: row.get(7)?,
                    last_access: row.get(8)?,
                })
            })
            .map_err(|e| EngError::Internal(format!("query failed: {}", e)))?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| EngError::Internal(format!("failed to collect rows: {}", e)))
    }

    /// Count total tenants.
    pub fn count(&self) -> Result<usize, EngError> {
        let conn = self.lock()?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tenants", [], |row| row.get(0))
            .map_err(|e| EngError::Internal(format!("count query failed: {}", e)))?;
        Ok(count as usize)
    }

    /// Count active tenants.
    pub fn count_active(&self) -> Result<usize, EngError> {
        let conn = self.lock()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tenants WHERE status = 'active'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| EngError::Internal(format!("count query failed: {}", e)))?;
        Ok(count as usize)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, EngError> {
        self.conn
            .lock()
            .map_err(|_| EngError::Internal("failed to acquire registry lock".to_string()))
    }

    fn row_to_tenant(row: &rusqlite::Row<'_>) -> Result<TenantRow, EngError> {
        Ok(TenantRow {
            tenant_id: row
                .get(0)
                .map_err(|e| EngError::Internal(format!("failed to get tenant_id: {}", e)))?,
            user_id: row
                .get(1)
                .map_err(|e| EngError::Internal(format!("failed to get user_id: {}", e)))?,
            created_at: row
                .get(2)
                .map_err(|e| EngError::Internal(format!("failed to get created_at: {}", e)))?,
            status: TenantStatus::parse(
                row.get::<_, String>(3)
                    .map_err(|e| EngError::Internal(format!("failed to get status: {}", e)))?
                    .as_str(),
            )
            .unwrap_or(TenantStatus::Active),
            data_path: row
                .get(4)
                .map_err(|e| EngError::Internal(format!("failed to get data_path: {}", e)))?,
            schema_version: row
                .get(5)
                .map_err(|e| EngError::Internal(format!("failed to get schema_version: {}", e)))?,
            quota_bytes: row
                .get(6)
                .map_err(|e| EngError::Internal(format!("failed to get quota_bytes: {}", e)))?,
            quota_memories: row
                .get(7)
                .map_err(|e| EngError::Internal(format!("failed to get quota_memories: {}", e)))?,
            last_access: row
                .get(8)
                .map_err(|e| EngError::Internal(format!("failed to get last_access: {}", e)))?,
        })
    }
}

impl std::fmt::Debug for RegistryDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryDb")
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    #[test]
    fn test_insert_and_get() {
        let db = RegistryDb::open_memory().unwrap();
        let now = now_secs();

        let row = TenantRow {
            tenant_id: "tenant_1".to_string(),
            user_id: "user_1".to_string(),
            created_at: now,
            status: TenantStatus::Active,
            data_path: "/data/tenants/tenant_1".to_string(),
            schema_version: 1,
            quota_bytes: Some(1_000_000),
            quota_memories: Some(1000),
            last_access: now,
        };

        db.insert(&row).unwrap();

        let fetched = db.get_by_user_id("user_1").unwrap().unwrap();
        assert_eq!(fetched.tenant_id, "tenant_1");
        assert_eq!(fetched.user_id, "user_1");
        assert_eq!(fetched.status, TenantStatus::Active);
    }

    #[test]
    fn test_update_status() {
        let db = RegistryDb::open_memory().unwrap();
        let now = now_secs();

        let row = TenantRow {
            tenant_id: "tenant_1".to_string(),
            user_id: "user_1".to_string(),
            created_at: now,
            status: TenantStatus::Active,
            data_path: "/data/tenants/tenant_1".to_string(),
            schema_version: 1,
            quota_bytes: None,
            quota_memories: None,
            last_access: now,
        };

        db.insert(&row).unwrap();
        db.update_status("tenant_1", TenantStatus::Suspended)
            .unwrap();

        let fetched = db.get_by_tenant_id("tenant_1").unwrap().unwrap();
        assert_eq!(fetched.status, TenantStatus::Suspended);
    }

    #[test]
    fn test_list_and_count() {
        let db = RegistryDb::open_memory().unwrap();
        let now = now_secs();

        for i in 0..3 {
            let row = TenantRow {
                tenant_id: format!("tenant_{}", i),
                user_id: format!("user_{}", i),
                created_at: now,
                status: TenantStatus::Active,
                data_path: format!("/data/tenants/tenant_{}", i),
                schema_version: 1,
                quota_bytes: None,
                quota_memories: None,
                last_access: now,
            };
            db.insert(&row).unwrap();
        }

        assert_eq!(db.count().unwrap(), 3);
        assert_eq!(db.count_active().unwrap(), 3);
        assert_eq!(db.list().unwrap().len(), 3);
    }
}
