// AGENTS - Agent registration and management (ported from TS agents/)
use crate::db::Database;
use crate::{EngError, Result};
use rusqlite::{params, OptionalExtension};
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
    db: &Database,
    user_id: i64,
    name: &str,
    category: Option<&str>,
    description: Option<&str>,
    code_hash: Option<&str>,
) -> Result<InsertAgentResult> {
    let name = name.to_string();
    let category = category.map(|s| s.to_string());
    let description = description.map(|s| s.to_string());
    let code_hash = code_hash.map(|s| s.to_string());

    db.write(move |conn| {
        conn.query_row(
            "INSERT INTO agents (user_id, name, category, description, code_hash) VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id, trust_score, created_at",
            params![user_id, name, category, description, code_hash],
            |row| {
                Ok(InsertAgentResult {
                    id: row.get(0)?,
                    trust_score: row.get(1)?,
                    created_at: row.get(2)?,
                })
            },
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

pub async fn get_agent_by_id(db: &Database, id: i64, user_id: i64) -> Result<Option<AgentRow>> {
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, user_id, name, category, description, code_hash, trust_score, total_ops, successful_ops, failed_ops, guard_allows, guard_warns, guard_blocks, is_active, last_seen_at, revoked_at, revoke_reason, created_at FROM agents WHERE id = ?1 AND user_id = ?2",
            params![id, user_id],
            row_to_agent,
        )
        .optional()
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

pub async fn get_agent_by_name(db: &Database, name: &str, user_id: i64) -> Result<Option<AgentRow>> {
    let name = name.to_string();
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, user_id, name, category, description, code_hash, trust_score, total_ops, successful_ops, failed_ops, guard_allows, guard_warns, guard_blocks, is_active, last_seen_at, revoked_at, revoke_reason, created_at FROM agents WHERE name = ?1 AND user_id = ?2",
            params![name, user_id],
            row_to_agent,
        )
        .optional()
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

pub async fn list_agents(db: &Database, user_id: i64) -> Result<Vec<AgentRow>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, user_id, name, category, description, code_hash, trust_score, total_ops, successful_ops, failed_ops, guard_allows, guard_warns, guard_blocks, is_active, last_seen_at, revoked_at, revoke_reason, created_at FROM agents WHERE user_id = ?1 ORDER BY created_at DESC"
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let rows = stmt.query_map(params![user_id], row_to_agent)
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

pub async fn revoke_agent(db: &Database, id: i64, user_id: i64, reason: &str) -> Result<()> {
    let reason = reason.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE agents SET is_active = 0, revoked_at = datetime('now'), revoke_reason = ?1, trust_score = 0 WHERE id = ?2 AND user_id = ?3",
            params![reason, id, user_id],
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

pub async fn link_key_to_agent(db: &Database, agent_id: i64, key_id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE api_keys SET agent_id = ?1 WHERE id = ?2 AND user_id = ?3",
            params![agent_id, key_id, user_id],
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

pub async fn get_agent_executions(db: &Database, agent_id: i64, limit: i64) -> Result<Vec<AgentExecutionRow>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, action, target_type, target_id, details, execution_hash, signature, created_at FROM audit_log WHERE agent_id = ?1 ORDER BY created_at DESC LIMIT ?2"
        ).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let rows = stmt.query_map(params![agent_id, limit], |row| {
            Ok(AgentExecutionRow {
                id: row.get(0)?,
                action: row.get(1)?,
                target_type: row.get(2)?,
                target_id: row.get(3)?,
                details: row.get(4)?,
                execution_hash: row.get(5)?,
                signature: row.get(6)?,
                created_at: row.get(7)?,
            })
        }).map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

fn row_to_agent(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRow> {
    Ok(AgentRow {
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
        is_active: row.get::<_, i64>(13).map(|v| v != 0)?,
        last_seen_at: row.get(14)?,
        revoked_at: row.get(15)?,
        revoke_reason: row.get(16)?,
        created_at: row.get(17)?,
    })
}
