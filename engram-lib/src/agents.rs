// AGENTS - Agent registration and management (ported from TS agents/)
use crate::Result;
use libsql::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRow {
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
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
    pub revoke_reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAgentBody {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub code_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertAgentResult {
    pub id: i64,
    pub trust_score: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionRow {
    pub id: i64,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<i64>,
    pub details: Option<String>,
    pub execution_hash: Option<String>,
    pub signature: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPassport {
    pub agent_id: i64,
    pub user_id: i64,
    pub name: String,
    pub trust_score: f64,
    pub issued_at: String,
    pub expires_at: Option<String>,
    pub signature: String,
}

pub async fn insert_agent(
    conn: &Connection,
    user_id: i64,
    name: &str,
    category: Option<&str>,
    description: Option<&str>,
    code_hash: Option<&str>,
) -> Result<InsertAgentResult> {
    let mut rows = conn.query("INSERT INTO agents (user_id, name, category, description, code_hash) VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id, trust_score, created_at", libsql::params![user_id, name.to_string(), category.map(|s| s.to_string()), description.map(|s| s.to_string()), code_hash.map(|s| s.to_string())]).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| crate::EngError::Internal("no result from insert".into()))?;
    Ok(InsertAgentResult {
        id: row.get(0)?,
        trust_score: row.get(1)?,
        created_at: row.get(2)?,
    })
}

pub async fn get_agent_by_id(conn: &Connection, id: i64, user_id: i64) -> Result<Option<AgentRow>> {
    let mut rows = conn.query("SELECT id, user_id, name, category, description, code_hash, trust_score, total_ops, successful_ops, failed_ops, guard_allows, guard_warns, guard_blocks, is_active, last_seen_at, revoked_at, revoke_reason, created_at FROM agents WHERE id = ?1 AND user_id = ?2", libsql::params![id, user_id]).await?;
    match rows.next().await? {
        Some(r) => Ok(Some(row_to_agent(&r)?)),
        None => Ok(None),
    }
}

pub async fn get_agent_by_name(
    conn: &Connection,
    name: &str,
    user_id: i64,
) -> Result<Option<AgentRow>> {
    let mut rows = conn.query("SELECT id, user_id, name, category, description, code_hash, trust_score, total_ops, successful_ops, failed_ops, guard_allows, guard_warns, guard_blocks, is_active, last_seen_at, revoked_at, revoke_reason, created_at FROM agents WHERE name = ?1 AND user_id = ?2", libsql::params![name.to_string(), user_id]).await?;
    match rows.next().await? {
        Some(r) => Ok(Some(row_to_agent(&r)?)),
        None => Ok(None),
    }
}

pub async fn list_agents(conn: &Connection, user_id: i64) -> Result<Vec<AgentRow>> {
    let mut rows = conn.query("SELECT id, user_id, name, category, description, code_hash, trust_score, total_ops, successful_ops, failed_ops, guard_allows, guard_warns, guard_blocks, is_active, last_seen_at, revoked_at, revoke_reason, created_at FROM agents WHERE user_id = ?1 ORDER BY created_at DESC", libsql::params![user_id]).await?;
    let mut agents = Vec::new();
    while let Some(r) = rows.next().await? {
        agents.push(row_to_agent(&r)?);
    }
    Ok(agents)
}

pub async fn revoke_agent(conn: &Connection, id: i64, user_id: i64, reason: &str) -> Result<()> {
    conn.execute("UPDATE agents SET is_active = 0, revoked_at = datetime('now'), revoke_reason = ?1, trust_score = 0 WHERE id = ?2 AND user_id = ?3", libsql::params![reason.to_string(), id, user_id]).await?;
    Ok(())
}

pub async fn link_key_to_agent(
    conn: &Connection,
    agent_id: i64,
    key_id: i64,
    user_id: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE api_keys SET agent_id = ?1 WHERE id = ?2 AND user_id = ?3",
        libsql::params![agent_id, key_id, user_id],
    )
    .await?;
    Ok(())
}

pub async fn get_agent_executions(
    conn: &Connection,
    agent_id: i64,
    limit: i64,
) -> Result<Vec<AgentExecutionRow>> {
    let mut rows = conn.query("SELECT id, action, target_type, target_id, details, execution_hash, signature, created_at FROM audit_log WHERE agent_id = ?1 ORDER BY created_at DESC LIMIT ?2", libsql::params![agent_id, limit]).await?;
    let mut execs = Vec::new();
    while let Some(r) = rows.next().await? {
        execs.push(AgentExecutionRow {
            id: r.get(0)?,
            action: r.get(1)?,
            target_type: r.get(2)?,
            target_id: r.get(3)?,
            details: r.get(4)?,
            execution_hash: r.get(5)?,
            signature: r.get(6)?,
            created_at: r.get(7)?,
        });
    }
    Ok(execs)
}

fn row_to_agent(r: &libsql::Row) -> Result<AgentRow> {
    Ok(AgentRow {
        id: r.get(0)?,
        user_id: r.get(1)?,
        name: r.get(2)?,
        category: r.get(3)?,
        description: r.get(4)?,
        code_hash: r.get(5)?,
        trust_score: r.get(6)?,
        total_ops: r.get(7)?,
        successful_ops: r.get(8)?,
        failed_ops: r.get(9)?,
        guard_allows: r.get(10)?,
        guard_warns: r.get(11)?,
        guard_blocks: r.get(12)?,
        is_active: r.get::<i64>(13).map(|v| v != 0)?,
        last_seen_at: r.get(14)?,
        revoked_at: r.get(15)?,
        revoke_reason: r.get(16)?,
        created_at: r.get(17)?,
    })
}
