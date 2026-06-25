use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::{EngError, Result};

// Single source of truth for default quota values when no tenant_quotas
// row exists for a user. Keep `check_quota`, `TenantQuota::default`, and
// this constant in sync (MT-F22).
pub const DEFAULT_MAX_MEMORIES: i64 = 100_000;
pub const DEFAULT_MAX_SPACES: i64 = 10;
/// Default storage byte limit per tenant (1 GiB), matching the DDL default
/// on `tenant_quotas.storage_bytes_limit`.
pub const DEFAULT_STORAGE_BYTES_LIMIT: i64 = 1_073_741_824;

/// Serde default for `TenantQuota.storage_bytes_limit`.
fn default_storage_bytes_limit() -> i64 {
    DEFAULT_STORAGE_BYTES_LIMIT
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Full quota configuration row for a tenant. Used for admin CRUD.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantQuota {
    pub user_id: i64,
    pub max_memories: i64,
    pub max_conversations: i64,
    pub max_api_keys: i64,
    pub max_spaces: i64,
    pub max_memory_size_bytes: i64,
    /// Maximum total bytes across all artifacts for this tenant.
    #[serde(default = "default_storage_bytes_limit")]
    pub storage_bytes_limit: i64,
    pub rate_limit_override: Option<i64>,
}

impl Default for TenantQuota {
    fn default() -> Self {
        Self {
            user_id: 0,
            max_memories: 10000,
            max_conversations: 1000,
            max_api_keys: 10,
            max_spaces: 5,
            max_memory_size_bytes: 102400,
            storage_bytes_limit: DEFAULT_STORAGE_BYTES_LIMIT,
            rate_limit_override: None,
        }
    }
}

/// Live usage snapshot checked at request time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaStatus {
    pub user_id: i64,
    pub memory_count: i64,
    pub memory_limit: i64,
    pub spaces_count: i64,
    pub spaces_limit: i64,
    /// Total bytes consumed by artifacts in this tenant's shard.
    pub storage_bytes_used: i64,
    /// Configured storage byte limit from tenant_quotas (or default).
    pub storage_bytes_limit: i64,
    pub within_limits: bool,
}

// ---------------------------------------------------------------------------
// Admin CRUD -- tenant_quotas table
// ---------------------------------------------------------------------------

/// Insert or update the quota row for a user.
#[tracing::instrument(skip(db, quota), fields(user_id = quota.user_id))]
pub async fn upsert_quota(db: &Database, quota: &TenantQuota) -> Result<()> {
    let user_id = quota.user_id;
    let max_memories = quota.max_memories;
    let max_conversations = quota.max_conversations;
    let max_api_keys = quota.max_api_keys;
    let max_spaces = quota.max_spaces;
    let max_memory_size_bytes = quota.max_memory_size_bytes;
    let storage_bytes_limit = quota.storage_bytes_limit;
    let rate_limit_override = quota.rate_limit_override;

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO tenant_quotas \
                 (user_id, max_memories, max_conversations, max_api_keys, max_spaces, \
                  max_memory_size_bytes, storage_bytes_limit, rate_limit_override) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT(user_id) DO UPDATE SET \
                 max_memories = excluded.max_memories, \
                 max_conversations = excluded.max_conversations, \
                 max_api_keys = excluded.max_api_keys, \
                 max_spaces = excluded.max_spaces, \
                 max_memory_size_bytes = excluded.max_memory_size_bytes, \
                 storage_bytes_limit = excluded.storage_bytes_limit, \
                 rate_limit_override = excluded.rate_limit_override",
            params![
                user_id,
                max_memories,
                max_conversations,
                max_api_keys,
                max_spaces,
                max_memory_size_bytes,
                storage_bytes_limit,
                rate_limit_override,
            ],
        )?;
        Ok(())
    })
    .await
}

/// List all quota rows joined with usernames.
#[tracing::instrument(skip(db))]
pub async fn list_quotas(db: &Database) -> Result<Vec<(TenantQuota, String)>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT tq.user_id, tq.max_memories, tq.max_conversations, tq.max_api_keys, \
                 tq.max_spaces, tq.max_memory_size_bytes, tq.storage_bytes_limit, \
                 tq.rate_limit_override, u.username \
                 FROM tenant_quotas tq \
                 JOIN users u ON tq.user_id = u.id",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                TenantQuota {
                    user_id: row.get(0).unwrap_or(0),
                    max_memories: row.get(1).unwrap_or(10000),
                    max_conversations: row.get(2).unwrap_or(1000),
                    max_api_keys: row.get(3).unwrap_or(10),
                    max_spaces: row.get(4).unwrap_or(5),
                    max_memory_size_bytes: row.get(5).unwrap_or(102400),
                    storage_bytes_limit: row.get(6).unwrap_or(DEFAULT_STORAGE_BYTES_LIMIT),
                    rate_limit_override: row.get(7).ok(),
                },
                row.get::<_, String>(8).unwrap_or_default(),
            ))
        })?;

        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    })
    .await
}

// ---------------------------------------------------------------------------
// Runtime quota check -- live usage snapshot
// ---------------------------------------------------------------------------

/// Check current usage against tenant_quotas for the given user.
///
/// If no quota row exists for the user, a default quota is synthesised
/// (100 000 memories, 10 spaces, 1 GiB storage) without writing to the DB.
#[tracing::instrument(skip(db))]
pub async fn check_quota(db: &Database, user_id: i64) -> Result<QuotaStatus> {
    db.read(move |conn| {
        // Fetch quota limits (or use defaults).
        let (memory_limit, spaces_limit, stor_limit): (i64, i64, i64) = {
            let mut stmt = conn.prepare(
                "SELECT max_memories, max_spaces, storage_bytes_limit \
                     FROM tenant_quotas WHERE user_id = ?1",
            )?;

            let mut rows = stmt.query(params![user_id])?;

            match rows.next()? {
                Some(row) => {
                    let ml: i64 = row.get(0)?;
                    let sl: i64 = row.get(1)?;
                    let stl: i64 = row.get(2)?;
                    (ml, sl, stl)
                }
                None => (
                    DEFAULT_MAX_MEMORIES,
                    DEFAULT_MAX_SPACES,
                    DEFAULT_STORAGE_BYTES_LIMIT,
                ),
            }
        };

        // Count active memories. user_id was re-added to memories (migration 64)
        // restoring single-DB multi-user mode; without the predicate this both
        // miscounted and disclosed every tenant's volume in monolith mode.
        let memory_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memories \
                 WHERE is_forgotten = 0 AND is_latest = 1 AND user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;

        // Count spaces.
        let spaces_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM spaces WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;

        // Sum artifact storage bytes (artifacts.user_id present in both modes).
        let storage_bytes_used: i64 = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM artifacts WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;

        let within_limits = memory_count < memory_limit
            && spaces_count <= spaces_limit
            && storage_bytes_used < stor_limit;

        Ok(QuotaStatus {
            user_id,
            memory_count,
            memory_limit,
            spaces_count,
            spaces_limit,
            storage_bytes_used,
            storage_bytes_limit: stor_limit,
            within_limits,
        })
    })
    .await
}

/// Check and enforce content quotas inside an open write transaction.
///
/// Called from inside the `db.transaction()` closure on every memory write.
/// Because the write pool has `writer_count = 1`, this check and the
/// subsequent INSERT are serialized by the connection pool -- no concurrent
/// writer can pass the check between this read and that INSERT. This is
/// TOCTOU-safe for a single-node deployment.
///
/// Returns `Err(EngError::QuotaExceeded)` if either the content-bytes limit
/// or the memory-count limit would be exceeded by this write. Returns `Ok(())`
/// immediately if both limits are `None` (unlimited).
pub fn enforce_quota_in_tx(
    tx: &rusqlite::Transaction,
    quota: &crate::tenant::types::QuotaConfig,
    content_bytes: i64,
) -> Result<()> {
    if quota.content_bytes.is_none() && quota.memory_count.is_none() {
        return Ok(());
    }

    let (cur_bytes, cur_count): (i64, i64) = tx
        .query_row(
            "SELECT
                (SELECT value FROM tenant_state WHERE key = 'content_bytes'),
                (SELECT value FROM tenant_state WHERE key = 'memory_count')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| EngError::DatabaseMessage(format!("quota read failed: {e}")))?;

    if let Some(limit) = quota.content_bytes {
        if cur_bytes + content_bytes > limit {
            return Err(EngError::QuotaExceeded(format!(
                "content quota exceeded: {} + {} > {} bytes",
                cur_bytes, content_bytes, limit
            )));
        }
    }

    if let Some(limit) = quota.memory_count {
        if cur_count + 1 > limit {
            return Err(EngError::QuotaExceeded(format!(
                "memory count quota exceeded: {} + 1 > {} memories",
                cur_count, limit
            )));
        }
    }

    Ok(())
}

/// Gate an artifact upload on the tenant's storage byte limit.
///
/// Queries the artifact table in the tenant shard for current usage, then
/// checks against `DEFAULT_STORAGE_BYTES_LIMIT` (tenant shards lack the
/// `tenant_quotas` table -- the limit is admin-configurable in the main DB
/// but the default applies uniformly here).
///
/// Returns `Err(EngError::Forbidden)` if `current_usage + upload_bytes`
/// would exceed the limit.
#[tracing::instrument(skip(db), fields(upload_bytes))]
pub async fn enforce_storage_quota(db: &Database, upload_bytes: i64) -> Result<()> {
    let current_usage: i64 = db
        .read(move |conn| {
            Ok(conn.query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM artifacts",
                params![],
                |row| row.get(0),
            )?)
        })
        .await?;

    if current_usage + upload_bytes > DEFAULT_STORAGE_BYTES_LIMIT {
        return Err(EngError::Forbidden(format!(
            "storage quota exceeded: {} of {} bytes used, upload of {} would exceed limit",
            current_usage, DEFAULT_STORAGE_BYTES_LIMIT, upload_bytes
        )));
    }
    Ok(())
}

/// Read quota defaults from environment variables.
///
/// All variables default to `None` (unlimited) when unset, preserving
/// backward compatibility for existing tenants and bare deployments.
///
/// Environment variables:
/// - `KLEOS_DEFAULT_CONTENT_QUOTA_BYTES` -- max content bytes per tenant
/// - `KLEOS_DEFAULT_MEMORY_COUNT_QUOTA`  -- max memory rows per tenant
/// - `KLEOS_DEFAULT_DISK_QUOTA_BYTES`    -- max shard directory bytes per tenant
pub fn default_quota_from_env() -> crate::tenant::types::QuotaConfig {
    /// Parse a named environment variable as i64, returning None if absent or non-numeric.
    fn read_i64(key: &str) -> Option<i64> {
        std::env::var(key).ok().and_then(|v| v.parse::<i64>().ok())
    }

    crate::tenant::types::QuotaConfig {
        content_bytes: read_i64("KLEOS_DEFAULT_CONTENT_QUOTA_BYTES"),
        memory_count: read_i64("KLEOS_DEFAULT_MEMORY_COUNT_QUOTA"),
        disk_bytes: read_i64("KLEOS_DEFAULT_DISK_QUOTA_BYTES"),
    }
}

/// Record a usage event in the usage_events table.
#[tracing::instrument(skip(db, event_type))]
pub async fn record_usage(
    db: &Database,
    user_id: i64,
    agent_id: Option<i64>,
    event_type: &str,
    quantity: i64,
) -> Result<()> {
    let event_type = event_type.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO usage_events (user_id, agent_id, event_type, quantity) \
             VALUES (?1, ?2, ?3, ?4)",
            params![user_id, agent_id, event_type, quantity],
        )?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod enforce_quota_in_tx_tests {
    use super::*;
    use crate::tenant::types::QuotaConfig;
    use rusqlite::Connection;

    /// Set up an in-memory DB with the tenant_state table seeded to given values.
    fn setup_tenant_state(bytes: i64, count: i64) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tenant_state (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO tenant_state(key, value) VALUES ('content_bytes', 0);
            INSERT INTO tenant_state(key, value) VALUES ('memory_count', 0);",
        )
        .unwrap();
        conn.execute(
            "UPDATE tenant_state SET value = ?1 WHERE key = 'content_bytes'",
            rusqlite::params![bytes],
        )
        .unwrap();
        conn.execute(
            "UPDATE tenant_state SET value = ?1 WHERE key = 'memory_count'",
            rusqlite::params![count],
        )
        .unwrap();
        conn
    }

    /// Unlimited quota always passes regardless of content size.
    #[test]
    fn test_unlimited_quota_always_passes() {
        let conn = setup_tenant_state(999_999_999, 999_999);
        let quota = QuotaConfig::default();
        let tx = conn.unchecked_transaction().unwrap();
        assert!(enforce_quota_in_tx(&tx, &quota, 1_000_000).is_ok());
    }

    /// Content bytes quota allows write when under limit.
    #[test]
    fn test_content_bytes_allow() {
        let conn = setup_tenant_state(400, 5);
        let quota = QuotaConfig {
            content_bytes: Some(1000),
            memory_count: None,
            disk_bytes: None,
        };
        let tx = conn.unchecked_transaction().unwrap();
        assert!(enforce_quota_in_tx(&tx, &quota, 100).is_ok());
    }

    /// Content bytes quota rejects write that would exceed limit.
    #[test]
    fn test_content_bytes_deny() {
        let conn = setup_tenant_state(950, 5);
        let quota = QuotaConfig {
            content_bytes: Some(1000),
            memory_count: None,
            disk_bytes: None,
        };
        let tx = conn.unchecked_transaction().unwrap();
        let result = enforce_quota_in_tx(&tx, &quota, 100);
        assert!(matches!(result, Err(EngError::QuotaExceeded(_))));
    }

    /// Memory count quota rejects when at limit.
    #[test]
    fn test_memory_count_deny() {
        let conn = setup_tenant_state(100, 10);
        let quota = QuotaConfig {
            content_bytes: None,
            memory_count: Some(10),
            disk_bytes: None,
        };
        let tx = conn.unchecked_transaction().unwrap();
        let result = enforce_quota_in_tx(&tx, &quota, 50);
        assert!(matches!(result, Err(EngError::QuotaExceeded(_))));
    }

    /// Memory count quota allows when one below limit.
    #[test]
    fn test_memory_count_allow() {
        let conn = setup_tenant_state(100, 9);
        let quota = QuotaConfig {
            content_bytes: None,
            memory_count: Some(10),
            disk_bytes: None,
        };
        let tx = conn.unchecked_transaction().unwrap();
        assert!(enforce_quota_in_tx(&tx, &quota, 50).is_ok());
    }
}

#[cfg(test)]
mod env_quota_tests {
    use super::*;

    /// Env defaults: unset = None (unlimited).
    #[test]
    fn test_default_quota_from_env_unset() {
        std::env::remove_var("KLEOS_DEFAULT_CONTENT_QUOTA_BYTES");
        std::env::remove_var("KLEOS_DEFAULT_MEMORY_COUNT_QUOTA");
        std::env::remove_var("KLEOS_DEFAULT_DISK_QUOTA_BYTES");
        let q = default_quota_from_env();
        assert!(q.content_bytes.is_none());
        assert!(q.memory_count.is_none());
        assert!(q.disk_bytes.is_none());
    }

    /// Env defaults: set values are parsed.
    #[test]
    fn test_default_quota_from_env_set() {
        std::env::set_var("KLEOS_DEFAULT_CONTENT_QUOTA_BYTES", "1048576");
        let q = default_quota_from_env();
        assert_eq!(q.content_bytes, Some(1_048_576));
        std::env::remove_var("KLEOS_DEFAULT_CONTENT_QUOTA_BYTES");
    }
}
