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
///
/// Columns `deleting_at`, `deleted_at`, and `stuck_at` track timestamps for
/// the deprovision state machine. The `deletions_log` table provides an audit
/// trail of all deprovision operations. The `cluster_lock` table guards against
/// multi-node double-teardown (shared with E2 shard quota).
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
    last_access INTEGER NOT NULL,
    deleting_at TEXT,
    deleted_at TEXT,
    stuck_at TEXT,
    quota_content_bytes INTEGER,
    quota_memory_count INTEGER,
    quota_disk_bytes INTEGER,
    content_bytes_used INTEGER DEFAULT 0,
    memory_count_used INTEGER DEFAULT 0,
    disk_bytes_used INTEGER DEFAULT 0,
    last_synced_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_tenants_user_id ON tenants(user_id);
CREATE INDEX IF NOT EXISTS idx_tenants_last_access ON tenants(last_access);
CREATE INDEX IF NOT EXISTS idx_tenants_status ON tenants(status);

CREATE TABLE IF NOT EXISTS deletions_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    deprovision_id TEXT UNIQUE NOT NULL,
    admin_user_id INTEGER,
    target_user_id INTEGER NOT NULL,
    target_username TEXT NOT NULL,
    deleted_at TEXT NOT NULL,
    reason TEXT,
    archive_path TEXT,
    shard_skipped INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_deletions_log_user ON deletions_log(target_user_id);
CREATE INDEX IF NOT EXISTS idx_deletions_log_deleted_at ON deletions_log(deleted_at);

CREATE TABLE IF NOT EXISTS cluster_lock (
    node_id TEXT PRIMARY KEY,
    heartbeat TEXT NOT NULL,
    started_at TEXT NOT NULL
);
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

        // Migrate existing databases: add columns that may not exist on old schemas.
        // SQLite errors if the column already exists; ignore that expected error.
        // E1: deprovision state machine columns.
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN deleting_at TEXT;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN deleted_at TEXT;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN stuck_at TEXT;");
        // E2: quota tracking columns (configured limits + observed usage).
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN quota_content_bytes INTEGER;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN quota_memory_count INTEGER;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN quota_disk_bytes INTEGER;");
        let _ = conn
            .execute_batch("ALTER TABLE tenants ADD COLUMN content_bytes_used INTEGER DEFAULT 0;");
        let _ = conn
            .execute_batch("ALTER TABLE tenants ADD COLUMN memory_count_used INTEGER DEFAULT 0;");
        let _ =
            conn.execute_batch("ALTER TABLE tenants ADD COLUMN disk_bytes_used INTEGER DEFAULT 0;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN last_synced_at TEXT;");

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

        // Idempotent column additions (no-ops for fresh DBs, needed for upgrades).
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN deleting_at TEXT;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN deleted_at TEXT;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN stuck_at TEXT;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN quota_content_bytes INTEGER;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN quota_memory_count INTEGER;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN quota_disk_bytes INTEGER;");
        let _ = conn
            .execute_batch("ALTER TABLE tenants ADD COLUMN content_bytes_used INTEGER DEFAULT 0;");
        let _ = conn
            .execute_batch("ALTER TABLE tenants ADD COLUMN memory_count_used INTEGER DEFAULT 0;");
        let _ =
            conn.execute_batch("ALTER TABLE tenants ADD COLUMN disk_bytes_used INTEGER DEFAULT 0;");
        let _ = conn.execute_batch("ALTER TABLE tenants ADD COLUMN last_synced_at TEXT;");

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

        // Fetch the row that now occupies this tenant_id. With INSERT OR IGNORE
        // a hash collision (a different user_id already owning this tenant_id)
        // leaves the existing row in place; surface that as a typed Conflict
        // rather than attaching this user to another user's shard. With a
        // 128-bit tenant_id hash this is cryptographically improbable.
        drop(conn);
        match self.get_by_tenant_id(&row.tenant_id)? {
            Some(existing) if existing.user_id == row.user_id => Ok(existing),
            Some(existing) => Err(EngError::Conflict(format!(
                "tenant id {} already maps to a different user (got {}, requested {})",
                row.tenant_id, existing.user_id, row.user_id
            ))),
            None => Err(EngError::Internal(
                "tenant row disappeared after insert".to_string(),
            )),
        }
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

    // ── Deprovision query methods ─────────────────────────────────────────

    /// List tenants with a given status.
    pub fn list_by_status(&self, status: TenantStatus) -> Result<Vec<TenantRow>, EngError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT tenant_id, user_id, created_at, status, data_path,
                        schema_version, quota_bytes, quota_memories, last_access
                 FROM tenants WHERE status = ?1 ORDER BY created_at ASC",
            )
            .map_err(|e| EngError::Internal(format!("prepare list_by_status: {e}")))?;
        let rows = stmt
            .query_map([status.as_str()], |row| {
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
            .map_err(|e| EngError::Internal(format!("query list_by_status: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| EngError::Internal(format!("collect list_by_status: {e}")))
    }

    /// Atomically mark a tenant as Deleting, recording the timestamp.
    ///
    /// Only transitions tenants that are currently Active or Suspended.
    /// Returns the number of rows affected (0 if already Deleting/Tombstone/Stuck).
    pub fn mark_deleting(&self, user_id: &str) -> Result<usize, EngError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        let affected = conn
            .execute(
                "UPDATE tenants SET status = 'deleting', deleting_at = ?1
                 WHERE user_id = ?2 AND status IN ('active', 'suspended')",
                rusqlite::params![now, user_id],
            )
            .map_err(|e| EngError::Internal(format!("mark_deleting: {e}")))?;
        Ok(affected)
    }

    /// Transition a tenant from Deleting to Tombstone.
    pub fn mark_tombstone(&self, tenant_id: &str) -> Result<(), EngError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "UPDATE tenants SET status = 'tombstone', deleted_at = ?1
             WHERE tenant_id = ?2 AND status = 'deleting'",
            rusqlite::params![now, tenant_id],
        )
        .map_err(|e| EngError::Internal(format!("mark_tombstone: {e}")))?;
        Ok(())
    }

    /// Transition a tenant from Deleting to Stuck after max failures.
    pub fn mark_stuck(&self, tenant_id: &str) -> Result<(), EngError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "UPDATE tenants SET status = 'stuck', stuck_at = ?1
             WHERE tenant_id = ?2 AND status = 'deleting'",
            rusqlite::params![now, tenant_id],
        )
        .map_err(|e| EngError::Internal(format!("mark_stuck: {e}")))?;
        Ok(())
    }

    /// Insert a row into deletions_log at the start of deprovision.
    pub fn insert_deletion_log(
        &self,
        deprovision_id: &str,
        admin_user_id: Option<i64>,
        target_user_id: i64,
        target_username: &str,
        reason: Option<&str>,
    ) -> Result<(), EngError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO deletions_log
             (deprovision_id, admin_user_id, target_user_id, target_username, deleted_at, reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                deprovision_id,
                admin_user_id,
                target_user_id,
                target_username,
                now,
                reason
            ],
        )
        .map_err(|e| EngError::Internal(format!("insert_deletion_log: {e}")))?;
        Ok(())
    }

    /// Fetch a deletions_log row by deprovision_id.
    pub fn get_deletion_log(
        &self,
        deprovision_id: &str,
    ) -> Result<Option<super::teardown::DeletionLogRow>, EngError> {
        use super::teardown::DeletionLogRow;
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT deprovision_id, admin_user_id, target_user_id, target_username,
                        deleted_at, reason, archive_path, shard_skipped
                 FROM deletions_log WHERE deprovision_id = ?1",
            )
            .map_err(|e| EngError::Internal(format!("prepare get_deletion_log: {e}")))?;
        let mut rows = stmt
            .query([deprovision_id])
            .map_err(|e| EngError::Internal(format!("query get_deletion_log: {e}")))?;
        match rows.next() {
            Ok(Some(row)) => Ok(Some(DeletionLogRow {
                deprovision_id: row.get(0).map_err(|e| EngError::Internal(e.to_string()))?,
                admin_user_id: row.get(1).map_err(|e| EngError::Internal(e.to_string()))?,
                target_user_id: row.get(2).map_err(|e| EngError::Internal(e.to_string()))?,
                target_username: row.get(3).map_err(|e| EngError::Internal(e.to_string()))?,
                deleted_at: row.get(4).map_err(|e| EngError::Internal(e.to_string()))?,
                reason: row.get(5).map_err(|e| EngError::Internal(e.to_string()))?,
                archive_path: row.get(6).map_err(|e| EngError::Internal(e.to_string()))?,
                shard_skipped: row
                    .get::<_, i64>(7)
                    .map_err(|e| EngError::Internal(e.to_string()))?
                    != 0,
            })),
            Ok(None) => Ok(None),
            Err(e) => Err(EngError::Internal(format!("get_deletion_log: {e}"))),
        }
    }

    /// Update the archive path in the deletions_log after archiving.
    pub fn update_deletion_log_archive(
        &self,
        deprovision_id: &str,
        archive_path: &str,
    ) -> Result<(), EngError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE deletions_log SET archive_path = ?1 WHERE deprovision_id = ?2",
            rusqlite::params![archive_path, deprovision_id],
        )
        .map_err(|e| EngError::Internal(format!("update_deletion_log_archive: {e}")))?;
        Ok(())
    }

    /// Mark the shard removal as skipped in the deletion log.
    pub fn update_deletion_log_shard_skipped(
        &self,
        deprovision_id: &str,
        note: &str,
    ) -> Result<(), EngError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE deletions_log SET shard_skipped = 1,
                    reason = COALESCE(reason, '') || ' [skip-shard: ' || ?2 || ']'
             WHERE deprovision_id = ?1",
            rusqlite::params![deprovision_id, note],
        )
        .map_err(|e| EngError::Internal(format!("update_deletion_log_shard_skipped: {e}")))?;
        Ok(())
    }

    /// Check if a username is under tombstone hold.
    ///
    /// Returns the hold-until date if the tombstone is still within the hold period.
    /// `hold_days` is typically read from `KLEOS_TOMBSTONE_HOLD_DAYS` (default 90).
    pub fn check_tombstone_hold(
        &self,
        user_id: &str,
        hold_days: i64,
    ) -> Result<Option<String>, EngError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT datetime(deleted_at, '+' || ?2 || ' days') FROM tenants
                 WHERE user_id = ?1 AND status = 'tombstone'
                 AND deleted_at >= datetime('now', '-' || ?2 || ' days')",
            )
            .map_err(|e| EngError::Internal(format!("prepare check_tombstone_hold: {e}")))?;
        let mut rows = stmt
            .query(rusqlite::params![user_id, hold_days])
            .map_err(|e| EngError::Internal(format!("query check_tombstone_hold: {e}")))?;
        match rows.next() {
            Ok(Some(row)) => Ok(Some(
                row.get(0).map_err(|e| EngError::Internal(e.to_string()))?,
            )),
            Ok(None) => Ok(None),
            Err(e) => Err(EngError::Internal(format!("check_tombstone_hold: {e}"))),
        }
    }

    /// Check if a username is under tombstone hold via the deletions_log.
    ///
    /// Used by the provision guard to prevent re-provisioning a recently deleted
    /// username. Returns the hold-until date if an active hold exists.
    pub fn check_tombstone_hold_by_username(
        &self,
        username: &str,
        hold_days: i64,
    ) -> Result<Option<String>, EngError> {
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT datetime(deleted_at, '+' || ?2 || ' days') FROM deletions_log
                 WHERE target_username = ?1
                 AND deleted_at >= datetime('now', '-' || ?2 || ' days')
                 ORDER BY deleted_at DESC LIMIT 1",
            )
            .map_err(|e| {
                EngError::Internal(format!("prepare check_tombstone_hold_by_username: {e}"))
            })?;
        let mut rows = stmt
            .query(rusqlite::params![username, hold_days])
            .map_err(|e| {
                EngError::Internal(format!("query check_tombstone_hold_by_username: {e}"))
            })?;
        match rows.next() {
            Ok(Some(row)) => Ok(Some(
                row.get(0).map_err(|e| EngError::Internal(e.to_string()))?,
            )),
            Ok(None) => Ok(None),
            Err(e) => Err(EngError::Internal(format!(
                "check_tombstone_hold_by_username: {e}"
            ))),
        }
    }

    /// Purge tombstones older than `hold_days`.
    ///
    /// Returns the number of rows permanently deleted.
    pub fn purge_expired_tombstones(&self, hold_days: i64) -> Result<usize, EngError> {
        let conn = self.lock()?;
        let affected = conn
            .execute(
                "DELETE FROM tenants
                 WHERE status = 'tombstone'
                 AND deleted_at < datetime('now', '-' || ?1 || ' days')",
                rusqlite::params![hold_days],
            )
            .map_err(|e| EngError::Internal(format!("purge_expired_tombstones: {e}")))?;
        Ok(affected)
    }

    /// List all deletions_log entries, most recent first.
    pub fn list_deletion_logs(
        &self,
        limit: usize,
    ) -> Result<Vec<super::teardown::DeletionLogRow>, EngError> {
        use super::teardown::DeletionLogRow;
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT deprovision_id, admin_user_id, target_user_id, target_username,
                        deleted_at, reason, archive_path, shard_skipped
                 FROM deletions_log ORDER BY deleted_at DESC LIMIT ?1",
            )
            .map_err(|e| EngError::Internal(format!("prepare list_deletion_logs: {e}")))?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(DeletionLogRow {
                    deprovision_id: row.get(0)?,
                    admin_user_id: row.get(1)?,
                    target_user_id: row.get(2)?,
                    target_username: row.get(3)?,
                    deleted_at: row.get(4)?,
                    reason: row.get(5)?,
                    archive_path: row.get(6)?,
                    shard_skipped: row.get::<_, i64>(7)? != 0,
                })
            })
            .map_err(|e| EngError::Internal(format!("query list_deletion_logs: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| EngError::Internal(format!("collect list_deletion_logs: {e}")))
    }

    /// List recent deletions_log entries (alias for `list_deletion_logs`).
    pub fn list_deletions_recent(
        &self,
        limit: i64,
    ) -> Result<Vec<super::teardown::DeletionLogRow>, EngError> {
        self.list_deletion_logs(limit as usize)
    }

    /// Find the most recent deletions_log row for a given tenant_id.
    ///
    /// Joins through the `tenants` table to map `tenant_id` to `target_user_id`.
    pub fn get_deletion_log_by_tenant(
        &self,
        tenant_id: &str,
    ) -> Result<Option<super::teardown::DeletionLogRow>, EngError> {
        use super::teardown::DeletionLogRow;
        let conn = self.lock()?;
        let mut stmt = conn
            .prepare(
                "SELECT dl.deprovision_id, dl.admin_user_id, dl.target_user_id,
                        dl.target_username, dl.deleted_at, dl.reason, dl.archive_path, dl.shard_skipped
                 FROM deletions_log dl
                 JOIN tenants t ON CAST(t.user_id AS INTEGER) = dl.target_user_id
                 WHERE t.tenant_id = ?1
                 ORDER BY dl.id DESC LIMIT 1",
            )
            .map_err(|e| EngError::Internal(format!("prepare get_deletion_log_by_tenant: {e}")))?;
        let mut rows = stmt
            .query([tenant_id])
            .map_err(|e| EngError::Internal(format!("query get_deletion_log_by_tenant: {e}")))?;
        match rows.next() {
            Ok(Some(row)) => Ok(Some(DeletionLogRow {
                deprovision_id: row.get(0).map_err(|e| EngError::Internal(e.to_string()))?,
                admin_user_id: row.get(1).map_err(|e| EngError::Internal(e.to_string()))?,
                target_user_id: row.get(2).map_err(|e| EngError::Internal(e.to_string()))?,
                target_username: row.get(3).map_err(|e| EngError::Internal(e.to_string()))?,
                deleted_at: row.get(4).map_err(|e| EngError::Internal(e.to_string()))?,
                reason: row.get(5).map_err(|e| EngError::Internal(e.to_string()))?,
                archive_path: row.get(6).map_err(|e| EngError::Internal(e.to_string()))?,
                shard_skipped: row
                    .get::<_, i64>(7)
                    .map_err(|e| EngError::Internal(e.to_string()))?
                    != 0,
            })),
            Ok(None) => Ok(None),
            Err(e) => Err(EngError::Internal(format!(
                "get_deletion_log_by_tenant: {e}"
            ))),
        }
    }

    // ── Cluster lock methods ──────────────────────────────────────────────

    /// Insert or update a cluster lock heartbeat for a node.
    pub fn cluster_lock_upsert(&self, node_id: &str) -> Result<(), EngError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "INSERT INTO cluster_lock (node_id, heartbeat, started_at)
             VALUES (?1, ?2, ?2)
             ON CONFLICT(node_id) DO UPDATE SET heartbeat = ?2",
            rusqlite::params![node_id, now],
        )
        .map_err(|e| EngError::Internal(format!("cluster_lock_upsert: {e}")))?;
        Ok(())
    }

    /// Update the heartbeat timestamp for an existing cluster lock.
    pub fn cluster_lock_heartbeat(&self, node_id: &str) -> Result<(), EngError> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.lock()?;
        conn.execute(
            "UPDATE cluster_lock SET heartbeat = ?1 WHERE node_id = ?2",
            rusqlite::params![now, node_id],
        )
        .map_err(|e| EngError::Internal(format!("cluster_lock_heartbeat: {e}")))?;
        Ok(())
    }

    /// Release a cluster lock for a node.
    pub fn cluster_lock_release(&self, node_id: &str) -> Result<(), EngError> {
        let conn = self.lock()?;
        conn.execute(
            "DELETE FROM cluster_lock WHERE node_id = ?1",
            rusqlite::params![node_id],
        )
        .map_err(|e| EngError::Internal(format!("cluster_lock_release: {e}")))?;
        Ok(())
    }

    /// List cluster lock entries held by OTHER nodes with a heartbeat
    /// fresher than `stale_seconds` ago.
    pub fn cluster_lock_active_others(
        &self,
        this_node_id: &str,
        stale_seconds: i64,
    ) -> Result<Vec<super::teardown::ClusterLockRow>, EngError> {
        use super::teardown::ClusterLockRow;
        let conn = self.lock()?;
        let modifier = format!("-{stale_seconds} seconds");
        let mut stmt = conn
            .prepare(
                "SELECT node_id, heartbeat, started_at FROM cluster_lock
                 WHERE node_id != ?1 AND heartbeat >= datetime('now', ?2)",
            )
            .map_err(|e| EngError::Internal(format!("prepare cluster_lock_active_others: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![this_node_id, modifier], |row| {
                Ok(ClusterLockRow {
                    node_id: row.get(0)?,
                    heartbeat: row.get(1)?,
                    started_at: row.get(2)?,
                })
            })
            .map_err(|e| EngError::Internal(format!("query cluster_lock_active_others: {e}")))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| EngError::Internal(format!("collect cluster_lock_active_others: {e}")))
    }

    // ── End deprovision query methods ─────────────────────────────────────

    /// Batch update usage counters for multiple tenants.
    ///
    /// Executes one UPDATE per tenant row in the same connection lock acquisition.
    /// Missing tenant rows are silently skipped (UPDATE affects 0 rows).
    pub fn bulk_set_usage(
        &self,
        updates: Vec<crate::jobs::quota_sync::TenantUsageUpdate>,
    ) -> Result<(), EngError> {
        let conn = self.lock()?;
        for update in &updates {
            conn.execute(
                "UPDATE tenants SET \
                    content_bytes_used = ?1, \
                    memory_count_used  = ?2, \
                    disk_bytes_used    = ?3, \
                    last_synced_at     = datetime('now') \
                 WHERE user_id = ?4",
                rusqlite::params![
                    update.content_bytes,
                    update.memory_count,
                    update.disk_bytes,
                    update.user_id,
                ],
            )
            .map_err(|e| {
                EngError::Internal(format!("bulk_set_usage failed for {}: {e}", update.user_id))
            })?;
        }
        Ok(())
    }

    /// Update configured quota limits for a tenant row.
    pub fn update_quota(
        &self,
        user_id: &str,
        content_bytes: Option<i64>,
        memory_count: Option<i64>,
        disk_bytes: Option<i64>,
    ) -> Result<(), EngError> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE tenants SET \
                quota_content_bytes = ?1, \
                quota_memory_count  = ?2, \
                quota_disk_bytes    = ?3 \
             WHERE user_id = ?4",
            rusqlite::params![content_bytes, memory_count, disk_bytes, user_id],
        )
        .map_err(|e| EngError::DatabaseMessage(format!("update_quota failed: {e}")))?;
        Ok(())
    }

    /// Read quota limits and shadow usage for a tenant row.
    pub fn get_quota_row(
        &self,
        user_id: &str,
    ) -> Result<crate::tenant::types::TenantQuotaRow, EngError> {
        let conn = self.lock()?;
        conn.query_row(
            "SELECT user_id, quota_content_bytes, quota_memory_count, quota_disk_bytes, \
                    COALESCE(content_bytes_used, 0), COALESCE(memory_count_used, 0), \
                    COALESCE(disk_bytes_used, 0), last_synced_at \
             FROM tenants WHERE user_id = ?1",
            rusqlite::params![user_id],
            |row| {
                Ok(crate::tenant::types::TenantQuotaRow {
                    user_id: row.get(0)?,
                    quota_content_bytes: row.get(1)?,
                    quota_memory_count: row.get(2)?,
                    quota_disk_bytes: row.get(3)?,
                    content_bytes_used: row.get(4)?,
                    memory_count_used: row.get(5)?,
                    disk_bytes_used: row.get(6)?,
                    last_synced_at: row.get(7)?,
                })
            },
        )
        .map_err(|e| EngError::NotFound(format!("tenant not found: {user_id}: {e}")))
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

    /// Helper to create a test tenant row.
    fn make_row(tenant_id: &str, user_id: &str, status: TenantStatus) -> TenantRow {
        TenantRow {
            tenant_id: tenant_id.to_string(),
            user_id: user_id.to_string(),
            created_at: now_secs(),
            status,
            data_path: format!("/data/tenants/{tenant_id}"),
            schema_version: 1,
            quota_bytes: None,
            quota_memories: None,
            last_access: now_secs(),
        }
    }

    #[test]
    fn test_mark_deleting_only_active_or_suspended() {
        let db = RegistryDb::open_memory().unwrap();
        db.insert(&make_row("t1", "u1", TenantStatus::Active))
            .unwrap();
        db.insert(&make_row("t2", "u2", TenantStatus::Suspended))
            .unwrap();

        // Active -> Deleting succeeds
        assert_eq!(db.mark_deleting("u1").unwrap(), 1);
        let fetched = db.get_by_user_id("u1").unwrap().unwrap();
        assert_eq!(fetched.status, TenantStatus::Deleting);

        // Suspended -> Deleting succeeds
        assert_eq!(db.mark_deleting("u2").unwrap(), 1);
        let fetched = db.get_by_user_id("u2").unwrap().unwrap();
        assert_eq!(fetched.status, TenantStatus::Deleting);

        // Already Deleting -> no-op (0 rows affected)
        assert_eq!(db.mark_deleting("u1").unwrap(), 0);
    }

    #[test]
    fn test_mark_tombstone_and_stuck() {
        let db = RegistryDb::open_memory().unwrap();
        db.insert(&make_row("t1", "u1", TenantStatus::Active))
            .unwrap();
        db.mark_deleting("u1").unwrap();

        // Deleting -> Tombstone
        db.mark_tombstone("t1").unwrap();
        let fetched = db.get_by_user_id("u1").unwrap().unwrap();
        assert_eq!(fetched.status, TenantStatus::Tombstone);

        // Set up another for Stuck
        db.insert(&make_row("t2", "u2", TenantStatus::Active))
            .unwrap();
        db.mark_deleting("u2").unwrap();
        db.mark_stuck("t2").unwrap();
        let fetched = db.get_by_user_id("u2").unwrap().unwrap();
        assert_eq!(fetched.status, TenantStatus::Stuck);
    }

    #[test]
    fn test_list_by_status() {
        let db = RegistryDb::open_memory().unwrap();
        db.insert(&make_row("t1", "u1", TenantStatus::Active))
            .unwrap();
        db.insert(&make_row("t2", "u2", TenantStatus::Suspended))
            .unwrap();
        db.insert(&make_row("t3", "u3", TenantStatus::Active))
            .unwrap();

        let active = db.list_by_status(TenantStatus::Active).unwrap();
        assert_eq!(active.len(), 2);
        let suspended = db.list_by_status(TenantStatus::Suspended).unwrap();
        assert_eq!(suspended.len(), 1);
        let deleting = db.list_by_status(TenantStatus::Deleting).unwrap();
        assert_eq!(deleting.len(), 0);
    }

    #[test]
    fn test_deletion_log_crud() {
        let db = RegistryDb::open_memory().unwrap();
        let dep_id = "dep-001";

        // Insert
        db.insert_deletion_log(dep_id, Some(42), 100, "alice", Some("policy violation"))
            .unwrap();

        // Fetch
        let log = db.get_deletion_log(dep_id).unwrap().unwrap();
        assert_eq!(log.deprovision_id, dep_id);
        assert_eq!(log.admin_user_id, Some(42));
        assert_eq!(log.target_user_id, 100);
        assert_eq!(log.target_username, "alice");
        assert_eq!(log.reason, Some("policy violation".to_string()));
        assert!(!log.shard_skipped);

        // Update archive path
        db.update_deletion_log_archive(dep_id, "/archives/dep-001.jsonl.gz")
            .unwrap();
        let log = db.get_deletion_log(dep_id).unwrap().unwrap();
        assert_eq!(
            log.archive_path,
            Some("/archives/dep-001.jsonl.gz".to_string())
        );

        // Mark shard skipped
        db.update_deletion_log_shard_skipped(dep_id, "admin override")
            .unwrap();
        let log = db.get_deletion_log(dep_id).unwrap().unwrap();
        assert!(log.shard_skipped);

        // List
        let all = db.list_deletion_logs(10).unwrap();
        assert_eq!(all.len(), 1);

        // Not found
        assert!(db.get_deletion_log("nonexistent").unwrap().is_none());
    }

    #[test]
    fn test_purge_expired_tombstones() {
        let db = RegistryDb::open_memory().unwrap();
        db.insert(&make_row("t1", "u1", TenantStatus::Active))
            .unwrap();
        db.mark_deleting("u1").unwrap();
        db.mark_tombstone("t1").unwrap();

        // With hold_days=0, tombstone should be immediately purgeable
        // (deleted_at is "now" which is < datetime('now', '-0 days'))
        // Actually datetime('now', '-0 days') == now, so equal is not < .
        // Use hold_days=0 -- the tombstone was just created, so it's NOT expired yet.
        let purged = db.purge_expired_tombstones(90).unwrap();
        assert_eq!(purged, 0, "fresh tombstone should not be purged");

        // Verify the tombstone still exists
        let row = db.get_by_user_id("u1").unwrap().unwrap();
        assert_eq!(row.status, TenantStatus::Tombstone);
    }

    /// Confirms E2 quota columns exist on the tenants table after open().
    #[test]
    fn quota_columns_exist_after_open() {
        let dir = tempfile::tempdir().unwrap();
        let db = RegistryDb::open(dir.path()).unwrap();
        let conn = db.conn.lock().unwrap();

        for col in [
            "quota_content_bytes",
            "quota_memory_count",
            "quota_disk_bytes",
            "content_bytes_used",
            "memory_count_used",
            "disk_bytes_used",
            "last_synced_at",
        ] {
            let exists: i64 = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM pragma_table_info('tenants') WHERE name='{col}'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "column {col} must exist on tenants table");
        }
    }
}
