use crate::db::Database;
#[cfg(feature = "db_pool")]
use crate::memory::uses_pool_backend;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub description: Option<String>,
    pub capabilities: serde_json::Value,
    pub status: String,
    pub config: serde_json::Value,
    pub heartbeat_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub quality_score: Option<f64>,
    pub drift_flags: serde_json::Value,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterAgentRequest {
    pub name: String,
    #[serde(rename = "type", alias = "agent_type", alias = "category")]
    pub type_: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub capabilities: Option<serde_json::Value>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SomaStats {
    pub total_agents: i64,
    pub online_agents: i64,
    pub types: i64,
}

const AGENT_COLUMNS: &str =
    "id, name, type, description, capabilities, status, config, heartbeat_at, \
     created_at, updated_at, quality_score, drift_flags, user_id";

const VALID_STATUSES: &[&str] = &["pending", "online", "offline", "error"];

fn parse_json(text: &str, fallback: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or(fallback)
}

fn row_to_agent(row: &libsql::Row) -> Result<Agent> {
    let capabilities_str: String = row.get(4)?;
    let config_str: String = row.get(6)?;
    let drift_flags_opt: Option<String> = row.get(11)?;
    Ok(Agent {
        id: row.get(0)?,
        name: row.get(1)?,
        type_: row.get(2)?,
        description: row.get(3)?,
        capabilities: parse_json(&capabilities_str, serde_json::json!([])),
        status: row.get(5)?,
        config: parse_json(&config_str, serde_json::json!({})),
        heartbeat_at: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        quality_score: row.get(10)?,
        drift_flags: drift_flags_opt
            .as_deref()
            .map(|s| parse_json(s, serde_json::json!([])))
            .unwrap_or_else(|| serde_json::json!([])),
        user_id: row.get(12)?,
    })
}

#[cfg(feature = "db_pool")]
fn row_to_agent_rusqlite(row: &rusqlite::Row<'_>) -> Result<Agent> {
    let capabilities_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let config_str: String = row.get(6).map_err(rusqlite_to_eng_error)?;
    let drift_flags_opt: Option<String> = row.get(11).map_err(rusqlite_to_eng_error)?;
    Ok(Agent {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        name: row.get(1).map_err(rusqlite_to_eng_error)?,
        type_: row.get(2).map_err(rusqlite_to_eng_error)?,
        description: row.get(3).map_err(rusqlite_to_eng_error)?,
        capabilities: parse_json(&capabilities_str, serde_json::json!([])),
        status: row.get(5).map_err(rusqlite_to_eng_error)?,
        config: parse_json(&config_str, serde_json::json!({})),
        heartbeat_at: row.get(7).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(8).map_err(rusqlite_to_eng_error)?,
        updated_at: row.get(9).map_err(rusqlite_to_eng_error)?,
        quality_score: row.get(10).map_err(rusqlite_to_eng_error)?,
        drift_flags: drift_flags_opt
            .as_deref()
            .map(|s| parse_json(s, serde_json::json!([])))
            .unwrap_or_else(|| serde_json::json!([])),
        user_id: row.get(12).map_err(rusqlite_to_eng_error)?,
    })
}

#[cfg(feature = "db_pool")]
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Register-or-upsert a soma agent by (user_id, name). Existing rows have
/// their type/description/capabilities/config overwritten so callers can
/// evolve an agent's registration without deleting the old row (and losing
/// the `agents.id` references held by soma_agent_groups / soma_agent_logs).
pub async fn register_agent(db: &Database, req: RegisterAgentRequest) -> Result<Agent> {
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id required".into()))?;
    if req.name.trim().is_empty() {
        return Err(EngError::InvalidInput("agent name required".into()));
    }
    if req.type_.trim().is_empty() {
        return Err(EngError::InvalidInput("agent type required".into()));
    }
    let capabilities = req
        .capabilities
        .clone()
        .unwrap_or_else(|| serde_json::json!([]));
    let config = req
        .config
        .clone()
        .unwrap_or_else(|| serde_json::json!({}));
    let capabilities_str = serde_json::to_string(&capabilities)?;
    let config_str = serde_json::to_string(&config)?;

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let name = req.name.clone();
        let type_ = req.type_.clone();
        let description = req.description.clone();
        db.write(move |conn| {
            conn.execute(
                "INSERT INTO soma_agents
                    (name, type, description, capabilities, config, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(name) DO UPDATE SET
                    type = excluded.type,
                    description = excluded.description,
                    capabilities = excluded.capabilities,
                    config = excluded.config,
                    updated_at = datetime('now')",
                rusqlite::params![
                    name,
                    type_,
                    description,
                    capabilities_str,
                    config_str,
                    user_id
                ],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await?;
        return get_agent_by_name(db, user_id, &req.name).await;
    }

    let conn = &db.conn;
    conn.execute(
        "INSERT INTO soma_agents
            (name, type, description, capabilities, config, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET
            type = excluded.type,
            description = excluded.description,
            capabilities = excluded.capabilities,
            config = excluded.config,
            updated_at = datetime('now')",
        libsql::params![
            req.name.clone(),
            req.type_.clone(),
            req.description.clone(),
            capabilities_str,
            config_str,
            user_id,
        ],
    )
    .await?;
    get_agent_by_name(db, user_id, &req.name).await
}

pub async fn heartbeat(db: &Database, agent_id: i64, user_id: i64) -> Result<()> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .write(move |conn| {
                conn.execute(
                    "UPDATE soma_agents
                     SET heartbeat_at = datetime('now'),
                         status = CASE WHEN status = 'offline' THEN 'online' ELSE status END,
                         updated_at = datetime('now')
                     WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![agent_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await;
    }

    let conn = &db.conn;
    conn.execute(
        "UPDATE soma_agents
         SET heartbeat_at = datetime('now'),
             status = CASE WHEN status = 'offline' THEN 'online' ELSE status END,
             updated_at = datetime('now')
         WHERE id = ?1 AND user_id = ?2",
        libsql::params![agent_id, user_id],
    )
    .await?;
    Ok(())
}

pub async fn set_status(db: &Database, agent_id: i64, user_id: i64, status: &str) -> Result<()> {
    if !VALID_STATUSES.contains(&status) {
        return Err(EngError::InvalidInput(format!(
            "invalid soma status '{}', must be one of pending, online, offline, error",
            status
        )));
    }

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let status = status.to_string();
        return db
            .write(move |conn| {
                conn.execute(
                    "UPDATE soma_agents SET status = ?1, updated_at = datetime('now')
                     WHERE id = ?2 AND user_id = ?3",
                    rusqlite::params![status, agent_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await;
    }

    let conn = &db.conn;
    conn.execute(
        "UPDATE soma_agents SET status = ?1, updated_at = datetime('now')
         WHERE id = ?2 AND user_id = ?3",
        libsql::params![status.to_string(), agent_id, user_id],
    )
    .await?;
    Ok(())
}

pub async fn list_agents(
    db: &Database,
    user_id: i64,
    type_filter: Option<&str>,
    status_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<Agent>> {
    let mut sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE user_id = ?1");
    let mut idx = 2usize;
    let mut params: Vec<libsql::Value> = vec![libsql::Value::Integer(user_id)];
    if let Some(t) = type_filter {
        sql.push_str(&format!(" AND type = ?{}", idx));
        params.push(libsql::Value::Text(t.to_string()));
        idx += 1;
    }
    if let Some(s) = status_filter {
        sql.push_str(&format!(" AND status = ?{}", idx));
        params.push(libsql::Value::Text(s.to_string()));
        idx += 1;
    }
    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", idx));
    params.push(libsql::Value::Integer(limit as i64));

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                let converted = rusqlite::params_from_iter(
                    params.iter().map(crate::memory::libsql_value_to_rusqlite_value),
                );
                let mut rows = stmt.query(converted).map_err(rusqlite_to_eng_error)?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    out.push(row_to_agent_rusqlite(row)?);
                }
                Ok(out)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn.query(&sql, libsql::params_from_iter(params)).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(row_to_agent(&row)?);
    }
    Ok(out)
}

pub async fn get_agent(db: &Database, id: i64, user_id: i64) -> Result<Agent> {
    let sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE id = ?1 AND user_id = ?2");

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![id, user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let row = rows
                    .next()
                    .map_err(rusqlite_to_eng_error)?
                    .ok_or_else(|| EngError::NotFound(format!("agent {}", id)))?;
                row_to_agent_rusqlite(row)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn.query(&sql, libsql::params![id, user_id]).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("agent {}", id)))?;
    row_to_agent(&row)
}

pub async fn get_agent_by_name(db: &Database, user_id: i64, name: &str) -> Result<Agent> {
    let sql =
        format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE user_id = ?1 AND name = ?2");

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let name_owned = name.to_string();
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![user_id, name_owned.clone()])
                    .map_err(rusqlite_to_eng_error)?;
                let row = rows
                    .next()
                    .map_err(rusqlite_to_eng_error)?
                    .ok_or_else(|| EngError::NotFound(format!("agent '{}'", name_owned)))?;
                row_to_agent_rusqlite(row)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(&sql, libsql::params![user_id, name.to_string()])
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("agent '{}'", name)))?;
    row_to_agent(&row)
}

pub async fn delete_agent(db: &Database, id: i64, user_id: i64) -> Result<()> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .write(move |conn| {
                conn.execute(
                    "DELETE FROM soma_agents WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await;
    }

    let conn = &db.conn;
    conn.execute(
        "DELETE FROM soma_agents WHERE id = ?1 AND user_id = ?2",
        libsql::params![id, user_id],
    )
    .await?;
    Ok(())
}

// --- Group types and functions (P0-0 Phase 27c) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLog {
    pub id: i64,
    pub agent_id: i64,
    pub level: String,
    pub message: String,
    pub data: Option<serde_json::Value>,
    pub created_at: String,
}

pub async fn create_group(
    db: &Database,
    name: String,
    description: Option<String>,
    user_id: i64,
) -> Result<Group> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let n = name.clone();
        let d = description.clone();
        db.write(move |conn| {
            conn.execute(
                "INSERT INTO soma_groups (name, description, user_id)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![n, d, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
        .await?;
        return get_group_by_name(db, &name, user_id).await;
    }

    let conn = &db.conn;
    conn.execute(
        "INSERT INTO soma_groups (name, description, user_id)
         VALUES (?1, ?2, ?3)",
        libsql::params![name.clone(), description.clone(), user_id],
    )
    .await?;
    get_group_by_name(db, &name, user_id).await
}

async fn get_group_by_name(db: &Database, name: &str, user_id: i64) -> Result<Group> {
    let sql = "SELECT id, name, description, user_id, created_at
               FROM soma_groups WHERE name = ?1 AND user_id = ?2";
    let n = name.to_string();

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![n.clone(), user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let row = rows
                    .next()
                    .map_err(rusqlite_to_eng_error)?
                    .ok_or_else(|| EngError::NotFound(format!("group '{}'", n)))?;
                Ok(Group {
                    id: row.get(0).map_err(rusqlite_to_eng_error)?,
                    name: row.get(1).map_err(rusqlite_to_eng_error)?,
                    description: row.get(2).map_err(rusqlite_to_eng_error)?,
                    user_id: row.get(3).map_err(rusqlite_to_eng_error)?,
                    created_at: row.get(4).map_err(rusqlite_to_eng_error)?,
                })
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn.query(sql, libsql::params![n.clone(), user_id]).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("group '{}'", n)))?;
    Ok(Group {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        user_id: row.get(3)?,
        created_at: row.get(4)?,
    })
}

pub async fn list_groups(db: &Database, user_id: i64) -> Result<Vec<Group>> {
    let sql = "SELECT id, name, description, user_id, created_at
               FROM soma_groups WHERE user_id = ?1 ORDER BY name ASC";

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    out.push(Group {
                        id: row.get(0).map_err(rusqlite_to_eng_error)?,
                        name: row.get(1).map_err(rusqlite_to_eng_error)?,
                        description: row.get(2).map_err(rusqlite_to_eng_error)?,
                        user_id: row.get(3).map_err(rusqlite_to_eng_error)?,
                        created_at: row.get(4).map_err(rusqlite_to_eng_error)?,
                    });
                }
                Ok(out)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn.query(sql, libsql::params![user_id]).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(Group {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            user_id: row.get(3)?,
            created_at: row.get(4)?,
        });
    }
    Ok(out)
}

pub async fn add_agent_to_group(
    db: &Database,
    agent_id: i64,
    group_id: i64,
    user_id: i64,
) -> Result<()> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .write(move |conn| {
                conn.execute(
                    "INSERT OR IGNORE INTO soma_agent_groups (agent_id, group_id, user_id)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![agent_id, group_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(())
            })
            .await;
    }

    let conn = &db.conn;
    conn.execute(
        "INSERT OR IGNORE INTO soma_agent_groups (agent_id, group_id, user_id)
         VALUES (?1, ?2, ?3)",
        libsql::params![agent_id, group_id, user_id],
    )
    .await?;
    Ok(())
}

pub async fn remove_agent_from_group(
    db: &Database,
    agent_id: i64,
    group_id: i64,
    user_id: i64,
) -> Result<bool> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let n = db
            .write(move |conn| {
                conn.execute(
                    "DELETE FROM soma_agent_groups
                     WHERE agent_id = ?1 AND group_id = ?2 AND user_id = ?3",
                    rusqlite::params![agent_id, group_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)
            })
            .await?;
        return Ok(n > 0);
    }

    let conn = &db.conn;
    let n = conn
        .execute(
            "DELETE FROM soma_agent_groups
             WHERE agent_id = ?1 AND group_id = ?2 AND user_id = ?3",
            libsql::params![agent_id, group_id, user_id],
        )
        .await?;
    Ok(n > 0)
}

pub async fn log_event(
    db: &Database,
    agent_id: i64,
    level: &str,
    message: &str,
    data: Option<serde_json::Value>,
) -> Result<i64> {
    let data_str = data
        .map(|d| serde_json::to_string(&d))
        .transpose()?;

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let l = level.to_string();
        let m = message.to_string();
        let ds = data_str.clone();
        return db
            .write(move |conn| {
                conn.execute(
                    "INSERT INTO soma_agent_logs (agent_id, level, message, data)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![agent_id, l, m, ds],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(conn.last_insert_rowid())
            })
            .await;
    }

    let conn = &db.conn;
    conn.execute(
        "INSERT INTO soma_agent_logs (agent_id, level, message, data)
         VALUES (?1, ?2, ?3, ?4)",
        libsql::params![agent_id, level.to_string(), message.to_string(), data_str],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    Ok(row.get(0)?)
}

pub async fn list_agent_logs(
    db: &Database,
    agent_id: i64,
    user_id: i64,
    limit: i64,
) -> Result<Vec<AgentLog>> {
    let sql = "SELECT l.id, l.agent_id, l.level, l.message, l.data, l.created_at
               FROM soma_agent_logs l
               JOIN soma_agents a ON l.agent_id = a.id
               WHERE l.agent_id = ?1 AND a.user_id = ?2
               ORDER BY l.created_at DESC LIMIT ?3";

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![agent_id, user_id, limit])
                    .map_err(rusqlite_to_eng_error)?;
                let mut out = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    let data_str: Option<String> = row.get(4).map_err(rusqlite_to_eng_error)?;
                    out.push(AgentLog {
                        id: row.get(0).map_err(rusqlite_to_eng_error)?,
                        agent_id: row.get(1).map_err(rusqlite_to_eng_error)?,
                        level: row.get(2).map_err(rusqlite_to_eng_error)?,
                        message: row.get(3).map_err(rusqlite_to_eng_error)?,
                        data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
                        created_at: row.get(5).map_err(rusqlite_to_eng_error)?,
                    });
                }
                Ok(out)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(sql, libsql::params![agent_id, user_id, limit])
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let data_str: Option<String> = row.get(4)?;
        out.push(AgentLog {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            level: row.get(2)?,
            message: row.get(3)?,
            data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
            created_at: row.get(5)?,
        });
    }
    Ok(out)
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<SomaStats> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let row = if let Some(uid) = user_id {
                    conn.query_row(
                        "SELECT
                            COUNT(*),
                            SUM(CASE WHEN status = 'online' THEN 1 ELSE 0 END),
                            COUNT(DISTINCT type)
                         FROM soma_agents WHERE user_id = ?1",
                        rusqlite::params![uid],
                        |row| {
                            let total: i64 = row.get(0)?;
                            let online: Option<i64> = row.get(1)?;
                            let types: i64 = row.get(2)?;
                            Ok((total, online.unwrap_or(0), types))
                        },
                    )
                    .map_err(rusqlite_to_eng_error)?
                } else {
                    conn.query_row(
                        "SELECT
                            COUNT(*),
                            SUM(CASE WHEN status = 'online' THEN 1 ELSE 0 END),
                            COUNT(DISTINCT type)
                         FROM soma_agents",
                        [],
                        |row| {
                            let total: i64 = row.get(0)?;
                            let online: Option<i64> = row.get(1)?;
                            let types: i64 = row.get(2)?;
                            Ok((total, online.unwrap_or(0), types))
                        },
                    )
                    .map_err(rusqlite_to_eng_error)?
                };
                Ok(SomaStats {
                    total_agents: row.0,
                    online_agents: row.1,
                    types: row.2,
                })
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = if let Some(uid) = user_id {
        conn.query(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN status = 'online' THEN 1 ELSE 0 END),
                COUNT(DISTINCT type)
             FROM soma_agents WHERE user_id = ?1",
            libsql::params![uid],
        )
        .await?
    } else {
        conn.query(
            "SELECT
                COUNT(*),
                SUM(CASE WHEN status = 'online' THEN 1 ELSE 0 END),
                COUNT(DISTINCT type)
             FROM soma_agents",
            (),
        )
        .await?
    };
    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no soma stats row".into()))?;
    let online: Option<i64> = row.get(1)?;
    Ok(SomaStats {
        total_agents: row.get(0)?,
        online_agents: online.unwrap_or(0),
        types: row.get(2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    #[tokio::test]
    async fn register_and_get_agent() {
        let db = setup().await;
        let a = register_agent(
            &db,
            RegisterAgentRequest {
                name: "claude-code".into(),
                type_: "llm".into(),
                description: Some("desktop coder".into()),
                capabilities: Some(serde_json::json!(["code", "memory"])),
                config: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        assert_eq!(a.name, "claude-code");
        assert_eq!(a.type_, "llm");
        let by_name = get_agent_by_name(&db, 1, "claude-code").await.unwrap();
        assert_eq!(by_name.id, a.id);
    }

    #[tokio::test]
    async fn register_upserts_existing() {
        let db = setup().await;
        let first = register_agent(
            &db,
            RegisterAgentRequest {
                name: "x".into(),
                type_: "old".into(),
                description: None,
                capabilities: None,
                config: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        let second = register_agent(
            &db,
            RegisterAgentRequest {
                name: "x".into(),
                type_: "new".into(),
                description: Some("updated".into()),
                capabilities: Some(serde_json::json!(["more"])),
                config: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.type_, "new");
        assert_eq!(second.description.as_deref(), Some("updated"));
    }

    #[tokio::test]
    async fn heartbeat_sets_timestamp() {
        let db = setup().await;
        let a = register_agent(
            &db,
            RegisterAgentRequest {
                name: "hb".into(),
                type_: "t".into(),
                description: None,
                capabilities: None,
                config: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        assert!(a.heartbeat_at.is_none());
        heartbeat(&db, a.id, 1).await.unwrap();
        let after = get_agent(&db, a.id, 1).await.unwrap();
        assert!(after.heartbeat_at.is_some());
    }

    #[tokio::test]
    async fn list_is_scoped_by_user() {
        let db = setup().await;
        register_agent(
            &db,
            RegisterAgentRequest {
                name: "mine".into(),
                type_: "t".into(),
                description: None,
                capabilities: None,
                config: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        let other = list_agents(&db, 2, None, None, 100).await.unwrap();
        assert!(other.is_empty());
    }
}
