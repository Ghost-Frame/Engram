use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::Result;

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

pub async fn get_quota(db: &Database, user_id: i64) -> Result<Option<TenantQuota>> {
    let mut rows = db
        .conn
        .query(
            "SELECT user_id, max_memories, max_conversations, max_api_keys, max_spaces, max_memory_size_bytes, rate_limit_override FROM tenant_quotas WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        Ok(Some(TenantQuota {
            user_id: row.get(0).unwrap_or(0),
            max_memories: row.get(1).unwrap_or(10000),
            max_conversations: row.get(2).unwrap_or(1000),
            max_api_keys: row.get(3).unwrap_or(10),
            max_spaces: row.get(4).unwrap_or(5),
            max_memory_size_bytes: row.get(5).unwrap_or(102400),
            rate_limit_override: row.get(6).ok(),
        }))
    } else {
        Ok(None)
    }
}

pub async fn upsert_quota(db: &Database, quota: &TenantQuota) -> Result<()> {
    db.conn
        .execute(
            "INSERT INTO tenant_quotas (user_id, max_memories, max_conversations, max_api_keys, max_spaces, max_memory_size_bytes, rate_limit_override)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(user_id) DO UPDATE SET
                 max_memories = excluded.max_memories,
                 max_conversations = excluded.max_conversations,
                 max_api_keys = excluded.max_api_keys,
                 max_spaces = excluded.max_spaces,
                 max_memory_size_bytes = excluded.max_memory_size_bytes,
                 rate_limit_override = excluded.rate_limit_override",
            libsql::params![
                quota.user_id,
                quota.max_memories,
                quota.max_conversations,
                quota.max_api_keys,
                quota.max_spaces,
                quota.max_memory_size_bytes,
                quota.rate_limit_override
            ],
        )
        .await?;
    Ok(())
}

pub async fn list_quotas(db: &Database) -> Result<Vec<(TenantQuota, String)>> {
    let mut rows = db
        .conn
        .query(
            "SELECT tq.user_id, tq.max_memories, tq.max_conversations, tq.max_api_keys, tq.max_spaces, tq.max_memory_size_bytes, tq.rate_limit_override, u.username
             FROM tenant_quotas tq
             JOIN users u ON tq.user_id = u.id",
            (),
        )
        .await?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().await? {
        let q = TenantQuota {
            user_id: row.get(0).unwrap_or(0),
            max_memories: row.get(1).unwrap_or(10000),
            max_conversations: row.get(2).unwrap_or(1000),
            max_api_keys: row.get(3).unwrap_or(10),
            max_spaces: row.get(4).unwrap_or(5),
            max_memory_size_bytes: row.get(5).unwrap_or(102400),
            rate_limit_override: row.get(6).ok(),
        };
        let username: String = row.get(7).unwrap_or_default();
        result.push((q, username));
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Runtime quota check -- live usage snapshot
// ---------------------------------------------------------------------------

/// Check current usage against tenant_quotas for the given user.
///
/// If no quota row exists for the user, a default quota is synthesised
/// (100 000 memories, 10 spaces) without writing to the DB.
pub async fn check_quota(db: &Database, user_id: i64) -> Result<QuotaStatus> {
    let mut quota_rows = db
        .conn
        .query(
            "SELECT max_memories, max_spaces FROM tenant_quotas WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;

    let (memory_limit, spaces_limit): (i64, i64) =
        if let Some(row) = quota_rows.next().await? {
            let ml: i64 = row
                .get(0)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?;
            let sl: i64 = row
                .get(1)
                .map_err(|e| crate::EngError::Internal(e.to_string()))?;
            (ml, sl)
        } else {
            (100_000, 10)
        };

    let mut mem_rows = db
        .conn
        .query(
            "SELECT COUNT(*) FROM memories WHERE user_id = ?1 AND is_forgotten = 0 AND is_latest = 1",
            libsql::params![user_id],
        )
        .await?;
    let memory_count: i64 = if let Some(row) = mem_rows.next().await? {
        row.get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?
    } else {
        0
    };

    let mut space_rows = db
        .conn
        .query(
            "SELECT COUNT(*) FROM spaces WHERE user_id = ?1",
            libsql::params![user_id],
        )
        .await?;
    let spaces_count: i64 = if let Some(row) = space_rows.next().await? {
        row.get(0)
            .map_err(|e| crate::EngError::Internal(e.to_string()))?
    } else {
        0
    };

    let within_limits = memory_count < memory_limit && spaces_count <= spaces_limit;

    Ok(QuotaStatus {
        user_id,
        memory_count,
        memory_limit,
        spaces_count,
        spaces_limit,
        within_limits,
    })
}

/// Record a usage event in the usage_events table.
pub async fn record_usage(
    db: &Database,
    user_id: i64,
    agent_id: Option<i64>,
    event_type: &str,
    quantity: i64,
) -> Result<()> {
    db.conn
        .execute(
            "INSERT INTO usage_events (user_id, agent_id, event_type, quantity)
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![user_id, agent_id, event_type, quantity],
        )
        .await?;

    Ok(())
}
