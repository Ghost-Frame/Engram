use crate::db::Database;
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
     created_at, updated_at, quality_score, drift_flags";

const VALID_STATUSES: &[&str] = &["pending", "online", "offline", "error"];

fn parse_json(text: &str, fallback: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or(fallback)
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

fn row_to_agent(row: &rusqlite::Row<'_>, owner_user_id: i64) -> Result<Agent> {
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
        user_id: owner_user_id,
    })
}

/// Register-or-upsert a soma agent by name. Existing rows have their
/// type/description/capabilities/config overwritten so callers can evolve an
/// agent's registration without deleting the old row (and losing the `agents.id`
/// references held by soma_agent_groups / soma_agent_logs).
#[tracing::instrument(skip(db, req), fields(name = %req.name, type_ = %req.type_))]
pub async fn register_agent(db: &Database, req: RegisterAgentRequest) -> Result<Agent> {
    let user_id = req.user_id.unwrap_or(1);
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
    let config = req.config.clone().unwrap_or_else(|| serde_json::json!({}));
    let capabilities_str = serde_json::to_string(&capabilities)?;
    let config_str = serde_json::to_string(&config)?;

    let name = req.name.clone();
    let type_ = req.type_.clone();
    let description = req.description.clone();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO soma_agents
                (name, type, description, capabilities, config)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET
                type = excluded.type,
                description = excluded.description,
                capabilities = excluded.capabilities,
                config = excluded.config,
                updated_at = datetime('now')",
            rusqlite::params![name, type_, description, capabilities_str, config_str],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;
    get_agent_by_name(db, user_id, &req.name).await
}

#[tracing::instrument(skip(db), fields(agent_id, user_id))]
pub async fn heartbeat(db: &Database, agent_id: i64, _user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "UPDATE soma_agents
             SET heartbeat_at = datetime('now'),
                 status = CASE WHEN status = 'offline' THEN 'online' ELSE status END,
                 updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![agent_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent_id, user_id, status = %status))]
pub async fn set_status(db: &Database, agent_id: i64, _user_id: i64, status: &str) -> Result<()> {
    if !VALID_STATUSES.contains(&status) {
        return Err(EngError::InvalidInput(format!(
            "invalid soma status '{}', must be one of pending, online, offline, error",
            status
        )));
    }

    let status = status.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE soma_agents SET status = ?1, updated_at = datetime('now')
             WHERE id = ?2",
            rusqlite::params![status, agent_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id, type_filter = ?type_filter, status_filter = ?status_filter, limit))]
pub async fn list_agents(
    db: &Database,
    user_id: i64,
    type_filter: Option<&str>,
    status_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<Agent>> {
    let mut sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents");
    let mut clauses: Vec<String> = Vec::new();
    let mut idx = 1usize;
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(t) = type_filter {
        clauses.push(format!("type = ?{}", idx));
        params.push(rusqlite::types::Value::Text(t.to_string()));
        idx += 1;
    }
    if let Some(s) = status_filter {
        clauses.push(format!("status = ?{}", idx));
        params.push(rusqlite::types::Value::Text(s.to_string()));
        idx += 1;
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(&format!(" ORDER BY created_at DESC LIMIT ?{}", idx));
    params.push(rusqlite::types::Value::Integer(limit as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let converted = rusqlite::params_from_iter(params.iter().cloned());
        let mut rows = stmt.query(converted).map_err(rusqlite_to_eng_error)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            out.push(row_to_agent(row, user_id)?);
        }
        Ok(out)
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent_id = id, user_id))]
pub async fn get_agent(db: &Database, id: i64, user_id: i64) -> Result<Agent> {
    let sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE id = ?1");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("agent {}", id)))?;
        row_to_agent(row, user_id)
    })
    .await
}

#[tracing::instrument(skip(db), fields(user_id, name = %name))]
pub async fn get_agent_by_name(db: &Database, user_id: i64, name: &str) -> Result<Agent> {
    let sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE name = ?1");
    let name_owned = name.to_string();

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![name_owned.clone()])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("agent '{}'", name_owned)))?;
        row_to_agent(row, user_id)
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent_id = id, user_id))]
pub async fn delete_agent(db: &Database, id: i64, _user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM soma_agents WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
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

#[tracing::instrument(skip(db, description), fields(name = %name, user_id))]
pub async fn create_group(
    db: &Database,
    name: String,
    description: Option<String>,
    user_id: i64,
) -> Result<Group> {
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
    get_group_by_name(db, &name, user_id).await
}

async fn get_group_by_name(db: &Database, name: &str, user_id: i64) -> Result<Group> {
    let sql = "SELECT id, name, description, user_id, created_at
               FROM soma_groups WHERE name = ?1 AND user_id = ?2";
    let n = name.to_string();

    db.read(move |conn| {
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
    .await
}

#[tracing::instrument(skip(db), fields(user_id))]
pub async fn list_groups(db: &Database, user_id: i64) -> Result<Vec<Group>> {
    let sql = "SELECT id, name, description, user_id, created_at
               FROM soma_groups WHERE user_id = ?1 ORDER BY name ASC";

    db.read(move |conn| {
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
    .await
}

#[tracing::instrument(skip(db), fields(agent_id, group_id, user_id))]
pub async fn add_agent_to_group(
    db: &Database,
    agent_id: i64,
    group_id: i64,
    user_id: i64,
) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO soma_agent_groups (agent_id, group_id, user_id)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![agent_id, group_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent_id, group_id, user_id))]
pub async fn remove_agent_from_group(
    db: &Database,
    agent_id: i64,
    group_id: i64,
    user_id: i64,
) -> Result<bool> {
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
    Ok(n > 0)
}

#[tracing::instrument(skip(db, message, data), fields(agent_id, level = %level))]
pub async fn log_event(
    db: &Database,
    agent_id: i64,
    level: &str,
    message: &str,
    data: Option<serde_json::Value>,
) -> Result<i64> {
    let data_str = data.map(|d| serde_json::to_string(&d)).transpose()?;
    let l = level.to_string();
    let m = message.to_string();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO soma_agent_logs (agent_id, level, message, data)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![agent_id, l, m, data_str],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(conn.last_insert_rowid())
    })
    .await
}

#[tracing::instrument(skip(db), fields(agent_id, user_id, limit))]
pub async fn list_agent_logs(
    db: &Database,
    agent_id: i64,
    _user_id: i64,
    limit: i64,
) -> Result<Vec<AgentLog>> {
    let sql = "SELECT l.id, l.agent_id, l.level, l.message, l.data, l.created_at
               FROM soma_agent_logs l
               WHERE l.agent_id = ?1
               ORDER BY l.created_at DESC LIMIT ?2";

    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![agent_id, limit])
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
    .await
}

#[tracing::instrument(skip(db), fields(user_id = ?_user_id))]
pub async fn get_stats(db: &Database, _user_id: Option<i64>) -> Result<SomaStats> {
    db.read(move |conn| {
        let row = conn
            .query_row(
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
            .map_err(rusqlite_to_eng_error)?;
        Ok(SomaStats {
            total_agents: row.0,
            online_agents: row.1,
            types: row.2,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

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

    /// Phase 5.8 dropped user_id from soma_agents: tenant isolation is now
    /// at the database level. A shared in-memory DB no longer separates user
    /// 1 from user 2 on soma_agents. The tenant-aware form of this invariant
    /// lands in kleos-server/tests once Phase 4.2 wires the tenant-aware
    /// test harness.
    #[tokio::test]
    #[ignore]
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
