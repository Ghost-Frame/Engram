//! Service functions for the Soma agent registry.
//!
//! Provides create/read/update/delete and query operations over the
//! `soma_agents`, `soma_groups`, `soma_agent_groups`, and `soma_agent_logs`
//! tables. All database access goes through the [`Database`] connection pool;
//! callers never touch raw SQL.

use crate::db::Database;
use crate::services::axon::publish_internal;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

/// A registered Soma agent row, returned from all read operations.
///
/// `capabilities` and `config` are stored as JSON text in the database and
/// deserialized to [`serde_json::Value`] on read. `drift_flags` defaults to
/// an empty array when the column is NULL.
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

/// Input for [`register_agent`]. Fields map to the `soma_agents` columns;
/// absent optional fields default to empty JSON objects/arrays or `None`.
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

/// Per-category count breakdown used inside stats responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatBreakdown {
    pub name: String,
    pub count: i64,
}

/// Aggregate statistics returned by [`get_stats`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SomaStats {
    pub total_agents: i64,
    pub online_agents: i64,
    pub types: i64,
    #[serde(default)]
    pub by_type: Vec<StatBreakdown>,
    #[serde(default)]
    pub by_status: Vec<StatBreakdown>,
}

/// Ordered column list shared by every SELECT on `soma_agents`. Positional
/// indices in [`row_to_agent`] must match this order exactly.
const AGENT_COLUMNS: &str =
    "id, name, type, description, capabilities, status, config, heartbeat_at, \
     created_at, updated_at, quality_score, drift_flags";

/// Accepted values for the `status` column. Validated by [`set_status`].
const VALID_STATUSES: &[&str] = &["pending", "online", "offline", "error"];

/// Attempt to parse `text` as JSON; return `fallback` on parse failure.
/// Used to handle rows where a JSON column was written by an older schema
/// version or a non-Rust writer.
fn parse_json(text: &str, fallback: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or(fallback)
}

/// Convert a [`rusqlite::Error`] into [`EngError::DatabaseMessage`] so it can
/// propagate through the crate's `Result` type.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Map a raw rusqlite `Row` to an [`Agent`] struct. Column order must match
/// [`AGENT_COLUMNS`]. `owner_user_id` fills `Agent.user_id`; the column is not
/// selected (correctness comes from the always-applied `user_id` predicate, so
/// the value is the caller's authenticated id by construction).
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
                (name, type, description, capabilities, config, user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(name, user_id) DO UPDATE SET type = excluded.type,
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
    let agent = get_agent_by_name(db, user_id, &req.name).await?;

    let _ = publish_internal(
        db,
        "system",
        "soma",
        "agent.registered",
        serde_json::json!({
            "agent_id": agent.id,
            "name": &agent.name,
            "type": &agent.type_,
        }),
    )
    .await;

    Ok(agent)
}

/// Record a heartbeat for the agent identified by `agent_id`. Updates
/// `heartbeat_at` and `updated_at` to the current time.
///
/// When `status_override` is `Some`, the agent's status is set to that value
/// (validated against [`VALID_STATUSES`]). When `None`, the agent transitions
/// from `offline` back to `online` if applicable and keeps its current status
/// otherwise. This mirrors the legacy engram-ts/standalone behavior where the
/// heartbeat body may carry a fresh status (e.g. `"error"`, `"online"`).
#[tracing::instrument(skip(db), fields(agent_id, user_id, status = ?status_override))]
pub async fn heartbeat(
    db: &Database,
    agent_id: i64,
    user_id: i64,
    status_override: Option<&str>,
) -> Result<()> {
    if let Some(s) = status_override {
        if !VALID_STATUSES.contains(&s) {
            return Err(EngError::InvalidInput(format!(
                "invalid soma status '{}', must be one of pending, online, offline, error",
                s
            )));
        }
    }

    let status_owned = status_override.map(|s| s.to_string());
    db.write(move |conn| {
        match status_owned {
            Some(status) => {
                conn.execute(
                    "UPDATE soma_agents
                     SET heartbeat_at = datetime('now'),
                         status = ?1,
                         updated_at = datetime('now')
                     WHERE id = ?2 AND user_id = ?3",
                    rusqlite::params![status, agent_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
            }
            None => {
                conn.execute(
                    "UPDATE soma_agents
                     SET heartbeat_at = datetime('now'),
                         status = CASE WHEN status = 'offline' THEN 'online' ELSE status END,
                         updated_at = datetime('now')
                     WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![agent_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
            }
        }
        Ok(())
    })
    .await
}

/// Set the `status` field of the agent identified by `agent_id`. Returns
/// [`EngError::InvalidInput`] when `status` is not one of the values in
/// [`VALID_STATUSES`].
#[tracing::instrument(skip(db), fields(agent_id, user_id, status = %status))]
pub async fn set_status(db: &Database, agent_id: i64, user_id: i64, status: &str) -> Result<()> {
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
             WHERE id = ?2 AND user_id = ?3",
            rusqlite::params![status, agent_id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// List agents owned by `user_id` with optional type and status filters.
/// `limit` caps the result set; callers should clamp to a sane maximum before
/// calling. The `user_id` predicate is always applied so the listing isolates
/// per user in single-DB mode (a no-op inside a single-owner shard).
#[tracing::instrument(skip(db), fields(user_id, type_filter = ?type_filter, status_filter = ?status_filter, limit))]
pub async fn list_agents(
    db: &Database,
    user_id: i64,
    type_filter: Option<&str>,
    status_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<Agent>> {
    // user_id is always the first bound parameter; type/status are appended.
    let mut clauses: Vec<String> = vec!["user_id = ?1".to_string()];
    let mut idx = 2usize;
    let mut params: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Integer(user_id)];
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
    let mut sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE ");
    sql.push_str(&clauses.join(" AND "));
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

/// Fetch the agent row for the given numeric `id` owned by `user_id`. Returns
/// [`EngError::NotFound`] when no such agent exists for that user. The
/// `user_id` predicate isolates the lookup per user in single-DB mode.
#[tracing::instrument(skip(db), fields(agent_id = id, user_id))]
pub async fn get_agent(db: &Database, id: i64, user_id: i64) -> Result<Agent> {
    let sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE id = ?1 AND user_id = ?2");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("agent {}", id)))?;
        row_to_agent(row, user_id)
    })
    .await
}

/// Fetch the agent row for the given `name` owned by `user_id`. Returns
/// [`EngError::NotFound`] when no agent with that name exists for that user.
/// The `(name, user_id)` lookup matches the table's UNIQUE(name, user_id).
#[tracing::instrument(skip(db), fields(user_id, name = %name))]
pub async fn get_agent_by_name(db: &Database, user_id: i64, name: &str) -> Result<Agent> {
    let sql = format!("SELECT {AGENT_COLUMNS} FROM soma_agents WHERE name = ?1 AND user_id = ?2");
    let name_owned = name.to_string();

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![name_owned.clone(), user_id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("agent '{}'", name_owned)))?;
        row_to_agent(row, user_id)
    })
    .await
}

/// Permanently delete the agent row identified by `id`. Does not cascade-delete
/// group membership or log rows; those are cleaned up by the database schema
/// via foreign-key constraints.
#[tracing::instrument(skip(db), fields(agent_id = id, user_id))]
pub async fn delete_agent(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM soma_agents WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    let _ = publish_internal(
        db,
        "system",
        "soma",
        "agent.deregistered",
        serde_json::json!({
            "agent_id": id,
        }),
    )
    .await;

    Ok(())
}

// --- Group types and functions (P0-0 Phase 27c) ---

/// An agent group row from `soma_groups`. Groups provide logical namespacing
/// for sets of agents within a tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub user_id: i64,
    pub created_at: String,
}

/// A single log entry from `soma_agent_logs`. `data` is an optional
/// structured JSON payload attached to the log line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLog {
    pub id: i64,
    pub agent_id: i64,
    pub level: String,
    pub message: String,
    pub data: Option<serde_json::Value>,
    pub created_at: String,
}

/// Create a new agent group named `name` owned by `user_id`. Returns the
/// full group row on success. Name collisions produce a database error (unique
/// constraint on `soma_groups.name`).
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

/// Private helper: fetch a group by name and owner. Used by [`create_group`]
/// to return the newly inserted row without a separate id lookup.
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

/// List all groups belonging to `user_id`, ordered alphabetically by name.
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

/// Fetch the group row for the given numeric `id` owned by `user_id`. Returns
/// [`EngError::NotFound`] when no such group exists for that tenant.
#[tracing::instrument(skip(db), fields(group_id = id, user_id))]
pub async fn get_group(db: &Database, id: i64, user_id: i64) -> Result<Group> {
    let sql = "SELECT id, name, description, user_id, created_at
               FROM soma_groups WHERE id = ?1 AND user_id = ?2";
    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("group {}", id)))?;
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

/// Return the agents owned by `user_id` that are members of `group_id`, ordered
/// alphabetically by agent name. The `a.user_id` predicate keeps the membership
/// listing scoped to the caller in single-DB mode.
#[tracing::instrument(skip(db), fields(group_id, user_id))]
pub async fn get_group_members(db: &Database, group_id: i64, user_id: i64) -> Result<Vec<Agent>> {
    let sql = format!(
        "SELECT a.{AGENT_COLUMNS} FROM soma_agents a
         INNER JOIN soma_agent_groups g ON g.agent_id = a.id
         WHERE g.group_id = ?1 AND a.user_id = ?2
         ORDER BY a.name ASC"
    );
    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![group_id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut agents = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            agents.push(row_to_agent(row, user_id)?);
        }
        Ok(agents)
    })
    .await
}

/// Add `agent_id` to `group_id` for the given `user_id`. The operation is
/// idempotent via `INSERT OR IGNORE`.
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

/// Remove `agent_id` from `group_id`. Returns `true` when the membership
/// row existed and was deleted, `false` when there was nothing to remove.
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

/// Append a log entry to the `soma_agent_logs` table for `agent_id`, but only
/// when that agent is owned by `user_id`. `soma_agent_logs` has no `user_id`
/// of its own, so the INSERT is guarded by an existence check against
/// `soma_agents` to prevent writing logs onto another user's agent in single-DB
/// mode. Returns the new row id, or [`EngError::NotFound`] when the agent is not
/// owned by the caller.
#[tracing::instrument(skip(db, message, data), fields(agent_id, user_id, level = %level))]
pub async fn log_event(
    db: &Database,
    agent_id: i64,
    user_id: i64,
    level: &str,
    message: &str,
    data: Option<serde_json::Value>,
) -> Result<i64> {
    let data_str = data.map(|d| serde_json::to_string(&d)).transpose()?;
    let l = level.to_string();
    let m = message.to_string();

    db.write(move |conn| {
        let inserted = conn
            .execute(
                "INSERT INTO soma_agent_logs (agent_id, level, message, data)
                 SELECT ?1, ?2, ?3, ?4
                 WHERE EXISTS (SELECT 1 FROM soma_agents WHERE id = ?1 AND user_id = ?5)",
                rusqlite::params![agent_id, l, m, data_str, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
        if inserted == 0 {
            return Err(EngError::NotFound(format!("agent {}", agent_id)));
        }
        Ok(conn.last_insert_rowid())
    })
    .await
}

/// Return the most recent `limit` log entries for `agent_id`, ordered newest
/// first. When `level` is `Some`, only entries with that exact level are
/// returned. Callers should clamp `limit` to a reasonable maximum before
/// calling.
#[tracing::instrument(skip(db), fields(agent_id, user_id, limit))]
pub async fn list_agent_logs(
    db: &Database,
    agent_id: i64,
    user_id: i64,
    limit: i64,
    level: Option<&str>,
) -> Result<Vec<AgentLog>> {
    let level_owned = level.map(|s| s.to_string());

    db.read(move |conn| {
        let mut out = Vec::new();
        // soma_agent_logs has no user_id; scope via the parent agent's owner so
        // one user cannot read another's agent logs by guessing an agent id.
        if let Some(ref lvl) = level_owned {
            let sql = "SELECT l.id, l.agent_id, l.level, l.message, l.data, l.created_at
                       FROM soma_agent_logs l
                       WHERE l.agent_id = ?1 AND l.level = ?2
                         AND l.agent_id IN (SELECT id FROM soma_agents WHERE user_id = ?4)
                       ORDER BY l.created_at DESC LIMIT ?3";
            let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![agent_id, lvl, limit, user_id])
                .map_err(rusqlite_to_eng_error)?;
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
        } else {
            let sql = "SELECT l.id, l.agent_id, l.level, l.message, l.data, l.created_at
                       FROM soma_agent_logs l
                       WHERE l.agent_id = ?1
                         AND l.agent_id IN (SELECT id FROM soma_agents WHERE user_id = ?3)
                       ORDER BY l.created_at DESC LIMIT ?2";
            let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![agent_id, limit, user_id])
                .map_err(rusqlite_to_eng_error)?;
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
        }
        Ok(out)
    })
    .await
}

/// Return every agent whose `status` is `online` but whose `heartbeat_at` is
/// older than the given `minutes` window. Used by external sweepers that decide
/// when to transition stale-online agents to offline.
///
/// `minutes` is clamped to the range [1, 1440] (1 minute to 24 hours). A value
/// of `0` becomes `1`; a value larger than `1440` becomes `1440`.
#[tracing::instrument(skip(db), fields(minutes = %minutes, user_id))]
pub async fn get_stale_agents(db: &Database, user_id: i64, minutes: i64) -> Result<Vec<Agent>> {
    let capped = minutes.clamp(1, 1440);
    let sql = format!(
        "SELECT {AGENT_COLUMNS} FROM soma_agents \
         WHERE user_id = ?2 \
           AND status = 'online' \
           AND heartbeat_at < datetime('now', '-' || ?1 || ' minutes')"
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![capped, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            results.push(row_to_agent(row, user_id)?);
        }
        Ok(results)
    })
    .await
}

/// Return every agent whose `capabilities` JSON array contains the given exact
/// `capability` string. A `LIKE` prefilter narrows the row set in SQLite;
/// an exact-match post-filter discards false positives where the capability
/// string is a substring of another entry (e.g. `"code"` must not match
/// `"code-review"`).
///
/// Returns an empty `Vec` when no agent matches.
#[tracing::instrument(skip(db), fields(capability = %capability, user_id))]
pub async fn find_by_capability(
    db: &Database,
    user_id: i64,
    capability: &str,
) -> Result<Vec<Agent>> {
    let needle = capability.to_string();
    let like_pattern = format!("%{capability}%");
    let sql = format!(
        "SELECT {AGENT_COLUMNS} FROM soma_agents WHERE user_id = ?2 AND capabilities LIKE ?1"
    );

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![like_pattern, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            let agent = row_to_agent(row, user_id)?;
            if let serde_json::Value::Array(ref arr) = agent.capabilities {
                if arr.iter().any(|v| v.as_str() == Some(needle.as_str())) {
                    results.push(agent);
                }
            }
        }
        Ok(results)
    })
    .await
}

/// Return aggregate statistics for the tenant's agent registry: total agent
/// count, count of agents currently `online`, and number of distinct agent
/// types.
#[tracing::instrument(skip(db), fields(user_id))]
pub async fn get_stats(db: &Database, user_id: i64) -> Result<SomaStats> {
    db.read(move |conn| {
        let row = conn
            .query_row(
                "SELECT
                    COUNT(*),
                    SUM(CASE WHEN status = 'online' THEN 1 ELSE 0 END),
                    COUNT(DISTINCT type)
                 FROM soma_agents WHERE user_id = ?1",
                rusqlite::params![user_id],
                |row| {
                    let total: i64 = row.get(0)?;
                    let online: Option<i64> = row.get(1)?;
                    let types: i64 = row.get(2)?;
                    Ok((total, online.unwrap_or(0), types))
                },
            )
            .map_err(rusqlite_to_eng_error)?;

        // by_type
        let mut by_type = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT type, COUNT(*) as cnt FROM soma_agents \
                 WHERE user_id = ?1 GROUP BY type ORDER BY cnt DESC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        while let Some(r) = rows.next().map_err(rusqlite_to_eng_error)? {
            by_type.push(StatBreakdown {
                name: r.get(0).map_err(rusqlite_to_eng_error)?,
                count: r.get(1).map_err(rusqlite_to_eng_error)?,
            });
        }

        // by_status
        let mut by_status = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT status, COUNT(*) as cnt FROM soma_agents \
                 WHERE user_id = ?1 GROUP BY status ORDER BY cnt DESC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;
        while let Some(r) = rows.next().map_err(rusqlite_to_eng_error)? {
            by_status.push(StatBreakdown {
                name: r.get(0).map_err(rusqlite_to_eng_error)?,
                count: r.get(1).map_err(rusqlite_to_eng_error)?,
            });
        }

        Ok(SomaStats {
            total_agents: row.0,
            online_agents: row.1,
            types: row.2,
            by_type,
            by_status,
        })
    })
    .await
}

/// Update the `quality_score` and/or `drift_flags` columns on an agent. Either
/// argument may be `None` to leave that column unchanged. `drift_flags` must be
/// a JSON array of strings; the service serializes it to text for storage.
///
/// Returns [`EngError::NotFound`] when no agent with `agent_id` exists in the
/// caller's shard, and [`EngError::InvalidInput`] when both fields are absent
/// (no-op write) or when `drift_flags` is not a JSON array.
#[tracing::instrument(skip(db, drift_flags), fields(agent_id, user_id))]
pub async fn update_agent_quality(
    db: &Database,
    agent_id: i64,
    user_id: i64,
    quality_score: Option<f64>,
    drift_flags: Option<serde_json::Value>,
) -> Result<Agent> {
    if quality_score.is_none() && drift_flags.is_none() {
        return Err(EngError::InvalidInput(
            "at least one of quality_score or drift_flags must be provided".into(),
        ));
    }
    if let Some(ref v) = drift_flags {
        if !v.is_array() {
            return Err(EngError::InvalidInput(
                "drift_flags must be a JSON array".into(),
            ));
        }
    }

    let drift_str = drift_flags
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    let changes = db
        .write(move |conn| {
            // Build an UPDATE that touches only the supplied columns.
            let mut sets: Vec<&'static str> = Vec::new();
            let mut params: Vec<rusqlite::types::Value> = Vec::new();
            let mut idx = 1usize;

            if let Some(q) = quality_score {
                sets.push("quality_score = ?");
                params.push(rusqlite::types::Value::Real(q));
                idx += 1;
            }
            if let Some(ref ds) = drift_str {
                sets.push("drift_flags = ?");
                params.push(rusqlite::types::Value::Text(ds.clone()));
                idx += 1;
            }
            sets.push("updated_at = datetime('now')");

            // Number each `?` placeholder positionally so SQLite can bind in order.
            let mut numbered = String::new();
            for (i, clause) in sets.iter().enumerate() {
                if i > 0 {
                    numbered.push_str(", ");
                }
                if clause.contains('?') {
                    numbered.push_str(&clause.replace('?', &format!("?{}", i + 1)));
                } else {
                    numbered.push_str(clause);
                }
            }
            params.push(rusqlite::types::Value::Integer(agent_id));
            params.push(rusqlite::types::Value::Integer(user_id));
            let sql = format!(
                "UPDATE soma_agents SET {} WHERE id = ?{} AND user_id = ?{}",
                numbered,
                idx,
                idx + 1
            );

            let converted = rusqlite::params_from_iter(params.iter().cloned());
            conn.execute(&sql, converted).map_err(rusqlite_to_eng_error)
        })
        .await?;

    if changes == 0 {
        return Err(EngError::NotFound(format!("agent id {}", agent_id)));
    }
    get_agent(db, agent_id, user_id).await
}

/// Delete the group with `group_id` and cascade-remove all of its membership
/// rows. Returns `true` when the group existed and was deleted, `false` when
/// no such group existed for `user_id`.
#[tracing::instrument(skip(db), fields(group_id, user_id))]
pub async fn delete_group(db: &Database, group_id: i64, user_id: i64) -> Result<bool> {
    let deleted = db
        .write(move |conn| {
            let tx = conn.transaction().map_err(rusqlite_to_eng_error)?;
            tx.execute(
                "DELETE FROM soma_agent_groups WHERE group_id = ?1 AND user_id = ?2",
                rusqlite::params![group_id, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            let n = tx
                .execute(
                    "DELETE FROM soma_groups WHERE id = ?1 AND user_id = ?2",
                    rusqlite::params![group_id, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
            tx.commit().map_err(rusqlite_to_eng_error)?;
            Ok(n)
        })
        .await?;
    Ok(deleted > 0)
}

/// Unit tests for the soma service layer. Each test spins up an in-memory
/// SQLite database so tests are isolated and require no external state.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// Create an in-memory database for use within a single test.
    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    /// Verify that registering an agent and fetching it by id or name returns
    /// consistent data.
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

    /// Verify that registering the same agent name twice updates the existing
    /// row rather than inserting a duplicate.
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

    /// Verify that calling `heartbeat` populates the `heartbeat_at` column.
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
        heartbeat(&db, a.id, 1, None).await.unwrap();
        let after = get_agent(&db, a.id, 1).await.unwrap();
        assert!(after.heartbeat_at.is_some());
    }

    /// Single-DB isolation: with user_id restored on soma_agents (monolith
    /// migration 67 / tenant v58), a shared in-memory DB again separates user 1
    /// from user 2. The cross-shard invariant is also covered by
    /// kleos-lib/tests/tenant_isolation.rs::soma_agents_isolated_across_tenants.
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
