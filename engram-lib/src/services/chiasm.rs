use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use crate::db::Database;
use crate::{EngError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Cancelled,
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskStatus::Pending => write!(f, "pending"),
            TaskStatus::InProgress => write!(f, "inprogress"),
            TaskStatus::Completed => write!(f, "completed"),
            TaskStatus::Blocked => write!(f, "blocked"),
            TaskStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl FromStr for TaskStatus {
    type Err = EngError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(TaskStatus::Pending),
            "inprogress" | "in_progress" => Ok(TaskStatus::InProgress),
            "completed" => Ok(TaskStatus::Completed),
            "blocked" => Ok(TaskStatus::Blocked),
            "cancelled" => Ok(TaskStatus::Cancelled),
            other => Err(EngError::InvalidInput(format!("unknown task status: {}", other))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: i32,
    pub agent: Option<String>,
    pub project: Option<String>,
    pub tags: Option<String>,       // JSON array stored as TEXT
    pub metadata: Option<String>,   // JSON object stored as TEXT
    pub user_id: i64,
    pub due_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub agent: Option<String>,
    pub project: Option<String>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
    pub user_id: Option<i64>,
    pub due_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTaskRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub agent: Option<String>,
    pub project: Option<String>,
    pub tags: Option<Vec<String>>,
    pub metadata: Option<serde_json::Value>,
    pub due_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChiasmStats {
    pub total: i64,
    pub by_status: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub priority: i32,
    pub agent: Option<String>,
    pub project: Option<String>,
    pub updated_at: String,
    pub created_at: String,
}

fn normalize_status_bucket(status: &str) -> String {
    match status {
        "pending" | "open" => "open".to_string(),
        "inprogress" | "in_progress" => "in_progress".to_string(),
        "completed" | "done" => "done".to_string(),
        other => other.to_string(),
    }
}

fn row_to_task(row: &libsql::Row) -> Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        status: row.get(3)?,
        priority: row.get(4)?,
        agent: row.get(5)?,
        project: row.get(6)?,
        tags: row.get(7)?,
        metadata: row.get(8)?,
        user_id: row.get(9)?,
        due_at: row.get(10)?,
        completed_at: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

pub async fn create_task(db: &Database, req: CreateTaskRequest) -> Result<Task> {
    let conn = &db.conn;

    let status = req.status.unwrap_or_else(|| "pending".to_string());
    let priority = req.priority.unwrap_or(5);
    let user_id = req.user_id.unwrap_or(1);
    let tags_json = req.tags.as_ref().map(serde_json::to_string).transpose()?;
    let metadata_json = req.metadata.as_ref().map(serde_json::to_string).transpose()?;

    conn.execute(
        "INSERT INTO tasks (title, description, status, priority, agent, project, tags, metadata, user_id, due_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        libsql::params![
            req.title,
            req.description,
            status,
            priority,
            req.agent,
            req.project,
            tags_json,
            metadata_json,
            user_id,
            req.due_at,
        ],
    )
    .await?;

    let mut rows = conn.query("SELECT last_insert_rowid()", ()).await?;
    let id_row = rows.next().await?.ok_or_else(|| EngError::Internal("no rowid".into()))?;
    let id: i64 = id_row.get(0)?;

    get_task(db, id).await
}

pub async fn get_task(db: &Database, id: i64) -> Result<Task> {
    let conn = &db.conn;
    let mut rows = conn.query(
        "SELECT id, title, description, status, priority, agent, project, tags, metadata,
                user_id, due_at, completed_at, created_at, updated_at
         FROM tasks WHERE id = ?1",
        libsql::params![id],
    )
    .await?;

    let row = rows
        .next()
        .await?
        .ok_or_else(|| EngError::NotFound(format!("task {}", id)))?;

    row_to_task(&row)
}

pub async fn list_tasks(
    db: &Database,
    user_id: Option<i64>,
    status: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<Task>> {
    let conn = &db.conn;

    let mut sql = String::from(
        "SELECT id, title, description, status, priority, agent, project, tags, metadata,
                user_id, due_at, completed_at, created_at, updated_at
         FROM tasks WHERE 1=1",
    );

    let mut param_idx = 1usize;
    let mut params_vec: Vec<libsql::Value> = Vec::new();

    if let Some(uid) = user_id {
        sql.push_str(&format!(" AND user_id = ?{}", param_idx));
        params_vec.push(libsql::Value::Integer(uid));
        param_idx += 1;
    }
    if let Some(s) = status {
        sql.push_str(&format!(" AND status = ?{}", param_idx));
        params_vec.push(libsql::Value::Text(s.to_string()));
        param_idx += 1;
    }

    sql.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ?{} OFFSET ?{}",
        param_idx,
        param_idx + 1
    ));
    params_vec.push(libsql::Value::Integer(limit as i64));
    params_vec.push(libsql::Value::Integer(offset as i64));

    let mut rows = conn.query(&sql, libsql::params_from_iter(params_vec)).await?;
    let mut results = Vec::new();
    while let Some(row) = rows.next().await? {
        results.push(row_to_task(&row)?);
    }
    Ok(results)
}

pub async fn update_task(db: &Database, id: i64, req: UpdateTaskRequest) -> Result<Task> {
    let conn = &db.conn;

    // Make sure the task exists first
    get_task(db, id).await?;

    let mut sets: Vec<String> = Vec::new();
    let mut params_vec: Vec<libsql::Value> = Vec::new();
    let mut idx = 1usize;

    if let Some(v) = req.title {
        sets.push(format!("title = ?{}", idx));
        params_vec.push(libsql::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = req.description {
        sets.push(format!("description = ?{}", idx));
        params_vec.push(libsql::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = &req.status {
        sets.push(format!("status = ?{}", idx));
        params_vec.push(libsql::Value::Text(v.clone()));
        idx += 1;
        // Auto-set completed_at when status becomes "completed"
        if v == "completed" {
            sets.push(format!("completed_at = ?{}", idx));
            params_vec.push(libsql::Value::Text("datetime('now')".to_string()));
            idx += 1;
        }
    }
    if let Some(v) = req.priority {
        sets.push(format!("priority = ?{}", idx));
        params_vec.push(libsql::Value::Integer(v as i64));
        idx += 1;
    }
    if let Some(v) = req.agent {
        sets.push(format!("agent = ?{}", idx));
        params_vec.push(libsql::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = req.project {
        sets.push(format!("project = ?{}", idx));
        params_vec.push(libsql::Value::Text(v));
        idx += 1;
    }
    if let Some(v) = req.tags {
        let j = serde_json::to_string(&v)?;
        sets.push(format!("tags = ?{}", idx));
        params_vec.push(libsql::Value::Text(j));
        idx += 1;
    }
    if let Some(v) = req.metadata {
        let j = serde_json::to_string(&v)?;
        sets.push(format!("metadata = ?{}", idx));
        params_vec.push(libsql::Value::Text(j));
        idx += 1;
    }
    if let Some(v) = req.due_at {
        sets.push(format!("due_at = ?{}", idx));
        params_vec.push(libsql::Value::Text(v));
        idx += 1;
    }

    // Always update updated_at
    sets.push(format!("updated_at = ?{}", idx));
    params_vec.push(libsql::Value::Text("datetime('now')".to_string()));
    idx += 1;

    if sets.is_empty() {
        return get_task(db, id).await;
    }

    let sql = format!("UPDATE tasks SET {} WHERE id = ?{}", sets.join(", "), idx);
    params_vec.push(libsql::Value::Integer(id));

    conn.execute(&sql, libsql::params_from_iter(params_vec)).await?;

    get_task(db, id).await
}

pub async fn delete_task(db: &Database, id: i64) -> Result<()> {
    let conn = &db.conn;
    conn.execute("DELETE FROM tasks WHERE id = ?1", libsql::params![id]).await?;
    Ok(())
}

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<ChiasmStats> {
    let conn = &db.conn;
    let mut by_status = BTreeMap::new();

    let mut rows = if let Some(uid) = user_id {
        conn.query(
            "SELECT status, COUNT(*) FROM tasks WHERE user_id = ?1 GROUP BY status",
            libsql::params![uid],
        )
        .await?
    } else {
        conn.query("SELECT status, COUNT(*) FROM tasks GROUP BY status", ())
            .await?
    };

    let mut total = 0i64;
    while let Some(row) = rows.next().await? {
        let status: String = row.get(0)?;
        let count: i64 = row.get(1)?;
        total += count;
        let bucket = normalize_status_bucket(&status);
        *by_status.entry(bucket).or_insert(0) += count;
    }

    Ok(ChiasmStats { total, by_status })
}

pub async fn get_feed(
    db: &Database,
    user_id: i64,
    limit: usize,
    offset: usize,
) -> Result<Vec<FeedItem>> {
    let conn = &db.conn;
    let mut rows = conn
        .query(
            "SELECT id, title, status, priority, agent, project, updated_at, created_at
             FROM tasks
             WHERE user_id = ?1
             ORDER BY updated_at DESC, created_at DESC
             LIMIT ?2 OFFSET ?3",
            libsql::params![user_id, limit as i64, offset as i64],
        )
        .await?;

    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(FeedItem {
            id: row.get(0)?,
            title: row.get(1)?,
            status: row.get(2)?,
            priority: row.get(3)?,
            agent: row.get(4)?,
            project: row.get(5)?,
            updated_at: row.get(6)?,
            created_at: row.get(7)?,
        });
    }

    Ok(items)
}
