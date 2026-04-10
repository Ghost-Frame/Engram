use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub code_hash: Option<String>,
    pub trust_score: f64,
    pub total_ops: i64,
    pub successful_ops: i64,
    pub failed_ops: i64,
    pub guard_allows: i64,
    pub guard_warns: i64,
    pub guard_blocks: i64,
    pub is_active: bool,
    pub revoked_at: Option<String>,
    pub revoke_reason: Option<String>,
    pub last_seen_at: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAgentRequest {
    pub user_id: Option<i64>,
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SomaStats {
    pub total_agents: i64,
    pub active_agents: i64,
    pub categories: i64,
}

fn row_to_agent(row: &libsql::Row) -> Result<Agent> {
    let is_active_int: i64 = row.get(13)?;
    Ok(Agent {
        id: row.get(0)?,
        user_id: row.get(1)?,
        name: row.get(2)?,
        category: row.get(3)?,
        description: row.get(4)?,
        code_hash: row.get(5)?,
        trust_score: row.get(6)?,
        total_ops: row.get(7)?,
        successful_ops: row.get(8)?,
        failed_ops: row.get(9)?,
        guard_allows: row.get(10)?,
        guard_warns: row.get(11)?,
        guard_blocks: row.get(12)?,
        is_active: is_active_int != 0,
        revoked_at: row.get(14)?,
        revoke_reason: row.get(15)?,
        last_seen_at: row.get(16)?,
        created_at: row.get(17)?,
    })
}

pub async fn register_agent(db: &Database, req: RegisterAgentRequest) -> Result<Agent> {
    let conn = &db.conn;
    let user_id = req.user_id.unwrap_or(1);

    // INSERT OR IGNORE -- if already exists (user_id+name conflict), just skip
    conn.execute(
        "INSERT OR IGNORE INTO agents (user_id, name, category, description)
         VALUES (?1, ?2, ?3, ?4)",
        libsql::params![user_id, req.name.clone(), req.category, req.description,],
    )
    .await?;

    get_agent_by_name(db, user_id, &req.name).await
}

pub async fn heartbeat(db: &Database, agent_id: i64, user_id: i64) -> Result<()> {
    let conn = &db.conn;
    conn.execute(
        "UPDATE agents SET last_seen_at = datetime('now') WHERE id = ?1 AND user_id = ?2",
        libsql::params![agent_id, user_id],
    )
    .await?;
    Ok(())
}

pub async fn list_agents(
    db: &Database,
    user_id: Option<i64>,
    active_only: bool,
) -> Result<Vec<Agent>> {
    let conn = &db.conn;

    let mut sql = String::from(
        "SELECT id, user_id, name, category, description, code_hash,
                trust_score, total_ops, successful_ops, failed_ops,
                guard_allows, guard_warns, guard_blocks, is_active,
                revoked_at, revoke_reason, last_seen_at, created_at
         FROM agents WHERE 1=1",
    );

    let mut param_idx = 1usize;
    let mut params_vec: Vec<libsql::Value> = Vec::new();

    if let Some(uid) = user_id {
        sql.push_str(&format!(" AND user_id = ?{}", param_idx));
        params_vec.push(libsql::Value::Integer(uid));
        param_idx += 1;
    }
    if active_only {
        sql.push_str(&format!(" AND is_active = ?{}", param_idx));
        params_vec.push(libsql::Value::Integer(1));
        param_idx += 1;
    }

    sql.push_str(" ORDER BY created_at DESC");
    let _ = param_idx; // suppress unused warning

    let mut rows = conn
        .query(&sql, libsql::params_from_iter(params_vec))
        .await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_agent(&row)?);
    }
    Ok(results)
}

pub async fn get_agent(db: &Database, id: i64, user_id: i64) -> Result<Agent> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, user_id, name, category, description, code_hash,
                trust_score, total_ops, successful_ops, failed_ops,
                guard_allows, guard_warns, guard_blocks, is_active,
                revoked_at, revoke_reason, last_seen_at, created_at
         FROM agents WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("agent {}", id)))?;

    row_to_agent(&row)
}

pub async fn get_agent_by_name(db: &Database, user_id: i64, name: &str) -> Result<Agent> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, user_id, name, category, description, code_hash,
                trust_score, total_ops, successful_ops, failed_ops,
                guard_allows, guard_warns, guard_blocks, is_active,
                revoked_at, revoke_reason, last_seen_at, created_at
         FROM agents WHERE user_id = ?1 AND name = ?2",
            libsql::params![user_id, name],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("agent '{}'", name)))?;

    row_to_agent(&row)
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<SomaStats> {
    let conn = &db.conn;
    let mut rows = if let Some(uid) = user_id {
        conn.query(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END),
                COUNT(DISTINCT category)
             FROM agents
             WHERE user_id = ?1",
            libsql::params![uid],
        )
        .await?
    } else {
        conn.query(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN is_active = 1 THEN 1 ELSE 0 END),
                COUNT(DISTINCT category)
             FROM agents",
            (),
        )
        .await?
    };

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no soma stats row".into()))?;
    let active_agents: Option<i64> = row.get(1)?;

    Ok(SomaStats {
        total_agents: row.get(0)?,
        active_agents: active_agents.unwrap_or(0),
        categories: row.get(2)?,
    })
}
