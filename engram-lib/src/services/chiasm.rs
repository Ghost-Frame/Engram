use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub agent: String,
    pub project: String,
    pub title: String,
    pub status: String,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskUpdate {
    pub id: i64,
    pub task_id: i64,
    pub agent: String,
    pub status: String,
    pub summary: Option<String>,
    pub created_at: String,
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub agent: String,
    pub project: String,
    pub title: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTaskRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChiasmStats {
    pub total: i64,
    pub by_status: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub id: i64,
    pub agent: String,
    pub project: String,
    pub title: String,
    pub status: String,
    pub summary: Option<String>,
    pub updated_at: String,
    pub created_at: String,
}

const TASK_COLUMNS: &str =
    "id, agent, project, title, status, summary, created_at, updated_at, user_id";

const VALID_STATUSES: &[&str] = &["active", "paused", "blocked", "completed"];

fn validate_status(status: &str) -> Result<()> {
    if VALID_STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(EngError::InvalidInput(format!(
            "invalid chiasm status '{}', must be one of active, paused, blocked, completed",
            status
        )))
    }
}

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

fn row_to_task(row: &rusqlite::Row<'_>) -> Result<Task> {
    Ok(Task {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
        project: row.get(2).map_err(rusqlite_to_eng_error)?,
        title: row.get(3).map_err(rusqlite_to_eng_error)?,
        status: row.get(4).map_err(rusqlite_to_eng_error)?,
        summary: row.get(5).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(6).map_err(rusqlite_to_eng_error)?,
        updated_at: row.get(7).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(8).map_err(rusqlite_to_eng_error)?,
    })
}

#[tracing::instrument(skip(db, req), fields(agent = %req.agent, project = ?req.project, title = %req.title))]
pub async fn create_task(db: &Database, req: CreateTaskRequest) -> Result<Task> {
    let status = req.status.clone().unwrap_or_else(|| "active".to_string());
    validate_status(&status)?;
    let user_id = req
        .user_id
        .ok_or_else(|| EngError::InvalidInput("user_id required".into()))?;

    let agent = req.agent.clone();
    let project = req.project.clone();
    let title = req.title.clone();
    let summary = req.summary.clone();
    let status_ins = status.clone();
    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO chiasm_tasks (agent, project, title, status, summary, user_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![agent, project, title, status_ins, summary, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    get_task(db, id, user_id).await
}

#[tracing::instrument(skip(db), fields(task_id = id, user_id))]
pub async fn get_task(db: &Database, id: i64, user_id: i64) -> Result<Task> {
    let sql = format!("SELECT {TASK_COLUMNS} FROM chiasm_tasks WHERE id = ?1 AND user_id = ?2");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("task {}", id)))?;
        row_to_task(row)
    })
    .await
}

pub async fn list_tasks(
    db: &Database,
    user_id: i64,
    status: Option<&str>,
    agent: Option<&str>,
    project: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<Task>> {
    let mut sql = format!("SELECT {TASK_COLUMNS} FROM chiasm_tasks WHERE user_id = ?1");
    let mut idx = 2usize;
    let mut params: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Integer(user_id)];

    if let Some(s) = status {
        sql.push_str(&format!(" AND status = ?{}", idx));
        params.push(rusqlite::types::Value::Text(s.to_string()));
        idx += 1;
    }
    if let Some(a) = agent {
        sql.push_str(&format!(" AND agent = ?{}", idx));
        params.push(rusqlite::types::Value::Text(a.to_string()));
        idx += 1;
    }
    if let Some(p) = project {
        sql.push_str(&format!(" AND project = ?{}", idx));
        params.push(rusqlite::types::Value::Text(p.to_string()));
        idx += 1;
    }
    sql.push_str(&format!(
        " ORDER BY updated_at DESC, id DESC LIMIT ?{} OFFSET ?{}",
        idx,
        idx + 1
    ));
    params.push(rusqlite::types::Value::Integer(limit as i64));
    params.push(rusqlite::types::Value::Integer(offset as i64));

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let converted = rusqlite::params_from_iter(params.iter().cloned());
        let mut rows = stmt.query(converted).map_err(rusqlite_to_eng_error)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            out.push(row_to_task(row)?);
        }
        Ok(out)
    })
    .await
}

/// Update a task AND append a history row atomically. The history row records
/// the agent making the change, the resulting status, and the resulting summary
/// so external consumers can replay the task's lifecycle.
pub async fn update_task(
    db: &Database,
    id: i64,
    req: UpdateTaskRequest,
    user_id: i64,
) -> Result<Task> {
    if let Some(ref s) = req.status {
        validate_status(s)?;
    }

    let req_for_tx = req.clone();
    db.transaction(move |tx| {
        let current: (String, String, Option<String>) = tx
            .query_row(
                "SELECT agent, status, summary FROM chiasm_tasks WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => EngError::NotFound(format!("task {}", id)),
                other => rusqlite_to_eng_error(other),
            })?;

        let new_title = req_for_tx.title.clone();
        let new_status = req_for_tx.status.clone().unwrap_or(current.1.clone());
        let new_summary = req_for_tx.summary.clone().or(current.2.clone());
        let new_agent = req_for_tx.agent.clone().unwrap_or(current.0.clone());

        let mut sets: Vec<&str> = Vec::new();
        let mut params_dyn: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(t) = new_title.as_ref() {
            sets.push("title = ?");
            params_dyn.push(Box::new(t.clone()));
        }
        if req_for_tx.status.is_some() {
            sets.push("status = ?");
            params_dyn.push(Box::new(new_status.clone()));
        }
        if req_for_tx.summary.is_some() {
            sets.push("summary = ?");
            params_dyn.push(Box::new(new_summary.clone()));
        }
        if req_for_tx.agent.is_some() {
            sets.push("agent = ?");
            params_dyn.push(Box::new(new_agent.clone()));
        }
        sets.push("updated_at = datetime('now')");

        let sql = format!(
            "UPDATE chiasm_tasks SET {} WHERE id = ? AND user_id = ?",
            sets.join(", ")
        );
        params_dyn.push(Box::new(id));
        params_dyn.push(Box::new(user_id));
        let refs: Vec<&dyn rusqlite::ToSql> = params_dyn.iter().map(|b| b.as_ref()).collect();
        tx.execute(&sql, refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;

        tx.execute(
            "INSERT INTO chiasm_task_updates (task_id, agent, status, summary, user_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, new_agent, new_status, new_summary, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;

        Ok(())
    })
    .await?;
    get_task(db, id, user_id).await
}

pub async fn delete_task(db: &Database, id: i64, user_id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM chiasm_tasks WHERE id = ?1 AND user_id = ?2",
            rusqlite::params![id, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

pub async fn list_task_history(
    db: &Database,
    task_id: i64,
    user_id: i64,
    limit: usize,
) -> Result<Vec<TaskUpdate>> {
    let sql = "SELECT id, task_id, agent, status, summary, created_at, user_id
               FROM chiasm_task_updates
               WHERE task_id = ?1 AND user_id = ?2
               ORDER BY id DESC
               LIMIT ?3";

    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![task_id, user_id, limit as i64])
            .map_err(rusqlite_to_eng_error)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            out.push(TaskUpdate {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                task_id: row.get(1).map_err(rusqlite_to_eng_error)?,
                agent: row.get(2).map_err(rusqlite_to_eng_error)?,
                status: row.get(3).map_err(rusqlite_to_eng_error)?,
                summary: row.get(4).map_err(rusqlite_to_eng_error)?,
                created_at: row.get(5).map_err(rusqlite_to_eng_error)?,
                user_id: row.get(6).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(out)
    })
    .await
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<ChiasmStats> {
    db.read(move |conn| {
        let mut by_status = BTreeMap::new();
        let mut total: i64 = 0;
        if let Some(uid) = user_id {
            let mut stmt = conn
                .prepare(
                    "SELECT status, COUNT(*) FROM chiasm_tasks WHERE user_id = ?1 GROUP BY status",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![uid])
                .map_err(rusqlite_to_eng_error)?;
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let s: String = row.get(0).map_err(rusqlite_to_eng_error)?;
                let c: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
                total += c;
                *by_status.entry(s).or_insert(0) += c;
            }
        } else {
            let mut stmt = conn
                .prepare("SELECT status, COUNT(*) FROM chiasm_tasks GROUP BY status")
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt.query([]).map_err(rusqlite_to_eng_error)?;
            while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let s: String = row.get(0).map_err(rusqlite_to_eng_error)?;
                let c: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
                total += c;
                *by_status.entry(s).or_insert(0) += c;
            }
        }
        Ok(ChiasmStats { total, by_status })
    })
    .await
}

pub async fn get_feed(
    db: &Database,
    user_id: i64,
    limit: usize,
    offset: usize,
) -> Result<Vec<FeedItem>> {
    let sql = "SELECT id, agent, project, title, status, summary, updated_at, created_at
               FROM chiasm_tasks
               WHERE user_id = ?1
               ORDER BY updated_at DESC, id DESC
               LIMIT ?2 OFFSET ?3";

    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id, limit as i64, offset as i64])
            .map_err(rusqlite_to_eng_error)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            out.push(FeedItem {
                id: row.get(0).map_err(rusqlite_to_eng_error)?,
                agent: row.get(1).map_err(rusqlite_to_eng_error)?,
                project: row.get(2).map_err(rusqlite_to_eng_error)?,
                title: row.get(3).map_err(rusqlite_to_eng_error)?,
                status: row.get(4).map_err(rusqlite_to_eng_error)?,
                summary: row.get(5).map_err(rusqlite_to_eng_error)?,
                updated_at: row.get(6).map_err(rusqlite_to_eng_error)?,
                created_at: row.get(7).map_err(rusqlite_to_eng_error)?,
            });
        }
        Ok(out)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    #[tokio::test]
    async fn create_and_get_task() {
        let db = setup().await;
        let t = create_task(
            &db,
            CreateTaskRequest {
                agent: "claude-code".into(),
                project: "engram-rust".into(),
                title: "port syntheos".into(),
                status: Some("active".into()),
                summary: Some("phase 27b".into()),
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        assert_eq!(t.status, "active");
        let fetched = get_task(&db, t.id, 1).await.unwrap();
        assert_eq!(fetched.title, "port syntheos");
    }

    #[tokio::test]
    async fn update_task_writes_history() {
        let db = setup().await;
        let t = create_task(
            &db,
            CreateTaskRequest {
                agent: "claude-code".into(),
                project: "engram-rust".into(),
                title: "t".into(),
                status: None,
                summary: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();

        update_task(
            &db,
            t.id,
            UpdateTaskRequest {
                title: None,
                status: Some("completed".into()),
                summary: Some("done".into()),
                agent: Some("claude-code".into()),
            },
            1,
        )
        .await
        .unwrap();

        let after = get_task(&db, t.id, 1).await.unwrap();
        assert_eq!(after.status, "completed");
        let history = list_task_history(&db, t.id, 1, 10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].status, "completed");
        assert_eq!(history[0].summary.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn list_is_scoped_by_user() {
        let db = setup().await;
        create_task(
            &db,
            CreateTaskRequest {
                agent: "a".into(),
                project: "p".into(),
                title: "mine".into(),
                status: None,
                summary: None,
                user_id: Some(1),
            },
        )
        .await
        .unwrap();
        let other = list_tasks(&db, 2, None, None, None, 10, 0).await.unwrap();
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn invalid_status_rejected() {
        let db = setup().await;
        let r = create_task(
            &db,
            CreateTaskRequest {
                agent: "a".into(),
                project: "p".into(),
                title: "t".into(),
                status: Some("nonsense".into()),
                summary: None,
                user_id: Some(1),
            },
        )
        .await;
        assert!(r.is_err());
    }
}
