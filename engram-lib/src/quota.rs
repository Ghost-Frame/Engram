use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::{EngError, Result};

// Single source of truth for default quota values when no tenant_quotas
// row exists for a user. Keep `check_quota`, `TenantQuota::default`, and
// this constant in sync (MT-F22).
pub const DEFAULT_MAX_MEMORIES: i64 = 100_000;
pub const DEFAULT_MAX_SPACES: i64 = 10;

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
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
    pub within_limits: bool,
}

// ---------------------------------------------------------------------------
// Admin CRUD -- tenant_quotas table
// ---------------------------------------------------------------------------

#[tracing::instrument(skip(db))]
pub async fn get_quota(db: &Database, user_id: i64) -> Result<Option<TenantQuota>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT user_id, max_memories, max_conversations, max_api_keys, max_spaces, \
                 max_memory_size_bytes, rate_limit_override \
                 FROM tenant_quotas WHERE user_id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;

        let mut rows = stmt
            .query(params![user_id])
            .map_err(rusqlite_to_eng_error)?;

        match rows.next().map_err(rusqlite_to_eng_error)? {
            Some(row) => Ok(Some(TenantQuota {
                user_id: row.get(0).unwrap_or(0),
                max_memories: row.get(1).unwrap_or(10000),
                max_conversations: row.get(2).unwrap_or(1000),
                max_api_keys: row.get(3).unwrap_or(10),
                max_spaces: row.get(4).unwrap_or(5),
                max_memory_size_bytes: row.get(5).unwrap_or(102400),
                rate_limit_override: row.get(6).ok(),
            })),
            None => Ok(None),
        }
    })
    .await
}

#[tracing::instrument(skip(db, quota), fields(user_id = quota.user_id))]
pub async fn upsert_quota(db: &Database, quota: &TenantQuota) -> Result<()> {
    let user_id = quota.user_id;
    let max_memories = quota.max_memories;
    let max_conversations = quota.max_conversations;
    let max_api_keys = quota.max_api_keys;
    let max_spaces = quota.max_spaces;
    let max_memory_size_bytes = quota.max_memory_size_bytes;
    let rate_limit_override = quota.rate_limit_override;

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO tenant_quotas \
                 (user_id, max_memories, max_conversations, max_api_keys, max_spaces, \
                  max_memory_size_bytes, rate_limit_override) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(user_id) DO UPDATE SET \
                 max_memories = excluded.max_memories, \
                 max_conversations = excluded.max_conversations, \
                 max_api_keys = excluded.max_api_keys, \
                 max_spaces = excluded.max_spaces, \
                 max_memory_size_bytes = excluded.max_memory_size_bytes, \
                 rate_limit_override = excluded.rate_limit_override",
            params![
                user_id,
                max_memories,
                max_conversations,
                max_api_keys,
                max_spaces,
                max_memory_size_bytes,
                rate_limit_override,
            ],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn list_quotas(db: &Database) -> Result<Vec<(TenantQuota, String)>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT tq.user_id, tq.max_memories, tq.max_conversations, tq.max_api_keys, \
                 tq.max_spaces, tq.max_memory_size_bytes, tq.rate_limit_override, u.username \
                 FROM tenant_quotas tq \
                 JOIN users u ON tq.user_id = u.id",
            )
            .map_err(rusqlite_to_eng_error)?;

        let rows = stmt
            .query_map([], |row| {
                Ok((
                    TenantQuota {
                        user_id: row.get(0).unwrap_or(0),
                        max_memories: row.get(1).unwrap_or(10000),
                        max_conversations: row.get(2).unwrap_or(1000),
                        max_api_keys: row.get(3).unwrap_or(10),
                        max_spaces: row.get(4).unwrap_or(5),
                        max_memory_size_bytes: row.get(5).unwrap_or(102400),
                        rate_limit_override: row.get(6).ok(),
                    },
                    row.get::<_, String>(7).unwrap_or_default(),
                ))
            })
            .map_err(rusqlite_to_eng_error)?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(rusqlite_to_eng_error)
    })
    .await
}

// ---------------------------------------------------------------------------
// Runtime quota check -- live usage snapshot
// ---------------------------------------------------------------------------

/// Check current usage against tenant_quotas for the given user.
///
/// If no quota row exists for the user, a default quota is synthesised
/// (100 000 memories, 10 spaces) without writing to the DB.
#[tracing::instrument(skip(db))]
pub async fn check_quota(db: &Database, user_id: i64) -> Result<QuotaStatus> {
    db.read(move |conn| {
        // Fetch quota limits (or use defaults).
        let (memory_limit, spaces_limit): (i64, i64) = {
            let mut stmt = conn
                .prepare("SELECT max_memories, max_spaces FROM tenant_quotas WHERE user_id = ?1")
                .map_err(rusqlite_to_eng_error)?;

            let mut rows = stmt
                .query(params![user_id])
                .map_err(rusqlite_to_eng_error)?;

            match rows.next().map_err(rusqlite_to_eng_error)? {
                Some(row) => {
                    let ml: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
                    let sl: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
                    (ml, sl)
                }
                None => (DEFAULT_MAX_MEMORIES, DEFAULT_MAX_SPACES),
            }
        };

        // Count active memories.
        let memory_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memories \
                 WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
                params![user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        // Count spaces.
        let spaces_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM spaces WHERE user_id = ?1",
                params![user_id],
                |row| row.get(0),
            )
            .map_err(rusqlite_to_eng_error)?;

        let within_limits = memory_count < memory_limit && spaces_count <= spaces_limit;

        Ok(QuotaStatus {
            user_id,
            memory_count,
            memory_limit,
            spaces_count,
            spaces_limit,
            within_limits,
        })
    })
    .await
}

/// Gate a memory write on tenant quota (MT-F20).
///
/// Call at the top of every handler that can create a new memory (store,
/// import, ingest, skill creation, etc.). Returns `Err(EngError::Forbidden)`
/// with a `quota_exceeded` marker when the tenant is already at or above
/// their limit, so callers can surface a 429/403 without leaking the
/// underlying numbers.
#[tracing::instrument(skip(db))]
pub async fn enforce_memory_quota(db: &Database, user_id: i64) -> Result<()> {
    let status = check_quota(db, user_id).await?;
    if !status.within_limits || status.memory_count >= status.memory_limit {
        return Err(EngError::Forbidden(format!(
            "quota exceeded: {} of {} memories used",
            status.memory_count, status.memory_limit
        )));
    }
    Ok(())
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
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}
