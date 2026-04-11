use crate::db::Database;
#[cfg(feature = "db_pool")]
use crate::memory::{libsql_value_to_rusqlite_value, uses_pool_backend};
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionEntry {
    pub id: i64,
    pub agent: String,
    pub action: String,
    pub summary: String,
    pub project: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub user_id: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogActionRequest {
    pub agent: String,
    pub action: String,
    pub summary: String,
    pub project: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrocaStats {
    pub total_actions: i64,
    pub agents: i64,
    pub projects: i64,
}

fn row_to_action_entry(row: &libsql::Row) -> Result<ActionEntry> {
    let metadata_str: Option<String> = row.get(4)?;
    let metadata = metadata_str
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    Ok(ActionEntry {
        id: row.get(0)?,
        agent: row.get(1)?,
        action: row.get(2)?,
        summary: row.get(3)?,
        metadata,
        project: row.get(5)?,
        user_id: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(feature = "db_pool")]
fn row_to_action_entry_rusqlite(row: &rusqlite::Row<'_>) -> Result<ActionEntry> {
    let metadata_str: Option<String> = row.get(4).map_err(rusqlite_to_eng_error)?;
    let metadata = metadata_str
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    Ok(ActionEntry {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
        action: row.get(2).map_err(rusqlite_to_eng_error)?,
        summary: row.get(3).map_err(rusqlite_to_eng_error)?,
        metadata,
        project: row.get(5).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(6).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(7).map_err(rusqlite_to_eng_error)?,
    })
}

#[cfg(feature = "db_pool")]
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

pub async fn log_action(db: &Database, req: LogActionRequest) -> Result<ActionEntry> {
    let metadata_str = req
        .metadata
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let user_id = req.user_id.unwrap_or(1);

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        let agent = req.agent;
        let action = req.action;
        let summary = req.summary;
        let project = req.project;
        let id = db
            .write(move |conn| {
                conn.execute(
                    "INSERT INTO action_log (agent, action, summary, project, metadata, user_id)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![agent, action, summary, project, metadata_str, user_id],
                )
                .map_err(rusqlite_to_eng_error)?;
                Ok(conn.last_insert_rowid())
            })
            .await?;
        return get_action(db, id, user_id).await;
    }

    let conn = &db.conn;

    conn.execute(
        "INSERT INTO action_log (agent, action, summary, project, metadata, user_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        libsql::params![
            req.agent,
            req.action,
            req.summary,
            req.project,
            metadata_str,
            user_id,
        ],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    // Fetch back the inserted row
    let mut rows = conn
        .query(
            "SELECT id, agent, action, summary, metadata, project, user_id, created_at
         FROM action_log WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("action_log row vanished".into()))?;

    row_to_action_entry(&row)
}

pub async fn query_actions(
    db: &Database,
    agent: Option<&str>,
    project: Option<&str>,
    action: Option<&str>,
    limit: usize,
    offset: usize,
    user_id: i64,
) -> Result<Vec<ActionEntry>> {
    let mut sql = String::from(
        "SELECT id, agent, action, summary, metadata, project, user_id, created_at
         FROM action_log WHERE user_id = ?1",
    );

    let mut param_idx = 2usize;
    let mut params_vec: Vec<libsql::Value> = vec![libsql::Value::Integer(user_id)];

    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(a.to_string()));
        param_idx += 1;
    }
    if let Some(p) = project {
        sql.push_str(&format!(" AND project = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(p.to_string()));
        param_idx += 1;
    }
    if let Some(act) = action {
        sql.push_str(&format!(" AND action = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(act.to_string()));
        param_idx += 1;
    }

    sql.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ?{} OFFSET ?{}",
        param_idx,
        param_idx + 1
    ));
    params_vec.push(libsql::Value::Integer(limit as i64));
    params_vec.push(libsql::Value::Integer(offset as i64));

    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
                let params = rusqlite::params_from_iter(
                    params_vec.iter().map(libsql_value_to_rusqlite_value),
                );
                let mut rows = stmt.query(params).map_err(rusqlite_to_eng_error)?;
                let mut results = Vec::new();
                while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                    results.push(row_to_action_entry_rusqlite(row)?);
                }
                Ok(results)
            })
            .await;
    }

    let conn = &db.conn;

    let mut rows = conn
        .query(&sql, libsql::params_from_iter(params_vec))
        .await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_action_entry(&row)?);
    }
    Ok(results)
}

pub async fn get_action(db: &Database, id: i64, user_id: i64) -> Result<ActionEntry> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, agent, action, summary, metadata, project, user_id, created_at
                         FROM action_log
                         WHERE id = ?1 AND user_id = ?2",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                let mut rows = stmt
                    .query(rusqlite::params![id, user_id])
                    .map_err(rusqlite_to_eng_error)?;
                let row = rows
                    .next()
                    .map_err(rusqlite_to_eng_error)?
                    .ok_or_else(|| EngError::NotFound(format!("action {}", id)))?;
                row_to_action_entry_rusqlite(row)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, agent, action, summary, metadata, project, user_id, created_at
             FROM action_log
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("action {}", id)))?;

    row_to_action_entry(&row)
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<BrocaStats> {
    #[cfg(feature = "db_pool")]
    if uses_pool_backend(db) {
        return db
            .read(move |conn| {
                let sql = if user_id.is_some() {
                    "SELECT
                        COUNT(*),
                        COUNT(DISTINCT agent),
                        COUNT(DISTINCT project)
                     FROM action_log
                     WHERE user_id = ?1"
                } else {
                    "SELECT
                        COUNT(*),
                        COUNT(DISTINCT agent),
                        COUNT(DISTINCT project)
                     FROM action_log"
                };

                let stats = if let Some(uid) = user_id {
                    conn.query_row(sql, rusqlite::params![uid], |row| {
                        Ok(BrocaStats {
                            total_actions: row.get(0)?,
                            agents: row.get(1)?,
                            projects: row.get(2)?,
                        })
                    })
                    .map_err(rusqlite_to_eng_error)?
                } else {
                    conn.query_row(sql, [], |row| {
                        Ok(BrocaStats {
                            total_actions: row.get(0)?,
                            agents: row.get(1)?,
                            projects: row.get(2)?,
                        })
                    })
                    .map_err(rusqlite_to_eng_error)?
                };

                Ok(stats)
            })
            .await;
    }

    let conn = &db.conn;
    let mut rows = if let Some(uid) = user_id {
        conn.query(
            "SELECT
                COUNT(*),
                COUNT(DISTINCT agent),
                COUNT(DISTINCT project)
             FROM action_log
             WHERE user_id = ?1",
            libsql::params![uid],
        )
        .await?
    } else {
        conn.query(
            "SELECT
                COUNT(*),
                COUNT(DISTINCT agent),
                COUNT(DISTINCT project)
             FROM action_log",
            (),
        )
        .await?
    };

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::Internal("no broca stats row".into()))?;

    Ok(BrocaStats {
        total_actions: row.get(0)?,
        agents: row.get(1)?,
        projects: row.get(2)?,
    })
}
