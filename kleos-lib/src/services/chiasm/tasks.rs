//! Chiasm task coordination service -- CRUD, history, stats, and activity feed
//! for multi-agent task management.

use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A Chiasm task representing a unit of work in multi-agent coordination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task identifier.
    pub id: i64,
    /// Agent responsible for this task.
    pub agent: String,
    /// Project this task belongs to.
    pub project: String,
    /// Short human-readable title.
    pub title: String,
    /// Current lifecycle status (active, paused, blocked, completed, blocked_on_human, stale, queued).
    pub status: String,
    /// Optional longer description or progress note.
    pub summary: Option<String>,
    /// Description of what the task should produce.
    pub expected_output: Option<String>,
    /// Format of the expected output (e.g. "json", "raw", "markdown").
    pub output_format: Option<String>,
    /// The actual output submitted by the agent.
    pub output: Option<String>,
    /// Precondition that must hold before the task can start.
    pub condition: Option<String>,
    /// External URL to validate output against (guardrail endpoint).
    pub guardrail_url: Option<String>,
    /// Number of times guardrail validation has been attempted.
    pub guardrail_retries: i64,
    /// LLM-generated execution plan for this task.
    pub plan: Option<String>,
    /// Feedback from a reviewer or guardrail rejection.
    pub feedback: Option<String>,
    /// Timestamp of the last heartbeat from the assigned agent.
    pub last_heartbeat: Option<String>,
    /// Expected interval (seconds) between heartbeats; used for stale detection.
    pub heartbeat_interval: i64,
    /// Whether the task has been assigned to an agent (false for queued/unassigned).
    pub assigned: bool,
    /// When the task was created.
    pub created_at: String,
    /// When the task was last modified.
    pub updated_at: String,
    /// Tenant user ID (shim for shard-level isolation).
    pub user_id: i64,
}

/// A single history entry recording a state transition for a Chiasm task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskUpdate {
    /// Unique update record identifier.
    pub id: i64,
    /// The task this update belongs to.
    pub task_id: i64,
    /// Agent that made this update.
    pub agent: String,
    /// Status after this update.
    pub status: String,
    /// Summary at the time of this update.
    pub summary: Option<String>,
    /// When the update was recorded.
    pub created_at: String,
    /// Tenant user ID.
    pub user_id: i64,
}

/// Request payload for creating a new Chiasm task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    /// Agent to assign the task to.
    pub agent: String,
    /// Project the task belongs to.
    pub project: String,
    /// Short human-readable title.
    pub title: String,
    /// Initial status (defaults to "active").
    #[serde(default)]
    pub status: Option<String>,
    /// Optional longer description.
    #[serde(default)]
    pub summary: Option<String>,
    /// Tenant user ID.
    #[serde(default)]
    pub user_id: Option<i64>,
    /// Description of expected output.
    #[serde(default)]
    pub expected_output: Option<String>,
    /// Format of expected output (defaults to "raw").
    #[serde(default)]
    pub output_format: Option<String>,
    /// Precondition for task start.
    #[serde(default)]
    pub condition: Option<String>,
    /// External guardrail validation URL.
    #[serde(default)]
    pub guardrail_url: Option<String>,
    /// Heartbeat interval in seconds (defaults to 300).
    #[serde(default)]
    pub heartbeat_interval: Option<i64>,
}

/// Request payload for partially updating an existing Chiasm task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTaskRequest {
    /// New title, if changing.
    #[serde(default)]
    pub title: Option<String>,
    /// New status, if changing.
    #[serde(default)]
    pub status: Option<String>,
    /// New summary, if changing.
    #[serde(default)]
    pub summary: Option<String>,
    /// New agent assignment, if changing.
    #[serde(default)]
    pub agent: Option<String>,
}

/// Aggregated statistics for the Chiasm task table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChiasmStats {
    /// Total number of tasks across all statuses.
    pub total: i64,
    /// Counts broken down by status value.
    pub by_status: BTreeMap<String, i64>,
}

/// A lightweight summary of a task for the activity feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedItem {
    /// Unique task identifier.
    pub id: i64,
    /// Agent responsible for this task.
    pub agent: String,
    /// Project this task belongs to.
    pub project: String,
    /// Short title.
    pub title: String,
    /// Current status.
    pub status: String,
    /// Optional summary text.
    pub summary: Option<String>,
    /// When the task was last modified.
    pub updated_at: String,
    /// When the task was created.
    pub created_at: String,
}

/// Column list for SELECT queries on chiasm_tasks.
const TASK_COLUMNS: &str = "id, agent, project, title, status, summary, \
    expected_output, output_format, output, condition, guardrail_url, \
    guardrail_retries, plan, feedback, last_heartbeat, heartbeat_interval, \
    assigned, created_at, updated_at";

/// All valid Chiasm task statuses.
const VALID_STATUSES: &[&str] = &[
    "active",
    "paused",
    "blocked",
    "completed",
    "blocked_on_human",
    "stale",
    "queued",
];

/// Validate that the given string is a recognized Chiasm task status.
fn validate_status(status: &str) -> Result<()> {
    if VALID_STATUSES.contains(&status) {
        Ok(())
    } else {
        Err(EngError::InvalidInput(format!(
            "invalid chiasm status '{}', must be one of: {}",
            status,
            VALID_STATUSES.join(", ")
        )))
    }
}

/// Map a rusqlite error to the crate's EngError type.
fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

/// Convert a database row to a Task struct.
fn row_to_task(row: &rusqlite::Row<'_>) -> Result<Task> {
    Ok(Task {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        agent: row.get(1).map_err(rusqlite_to_eng_error)?,
        project: row.get(2).map_err(rusqlite_to_eng_error)?,
        title: row.get(3).map_err(rusqlite_to_eng_error)?,
        status: row.get(4).map_err(rusqlite_to_eng_error)?,
        summary: row.get(5).map_err(rusqlite_to_eng_error)?,
        expected_output: row.get(6).map_err(rusqlite_to_eng_error)?,
        output_format: row.get(7).map_err(rusqlite_to_eng_error)?,
        output: row.get(8).map_err(rusqlite_to_eng_error)?,
        condition: row.get(9).map_err(rusqlite_to_eng_error)?,
        guardrail_url: row.get(10).map_err(rusqlite_to_eng_error)?,
        guardrail_retries: row.get::<_, i64>(11).map_err(rusqlite_to_eng_error)?,
        plan: row.get(12).map_err(rusqlite_to_eng_error)?,
        feedback: row.get(13).map_err(rusqlite_to_eng_error)?,
        last_heartbeat: row.get(14).map_err(rusqlite_to_eng_error)?,
        heartbeat_interval: row.get::<_, i64>(15).map_err(rusqlite_to_eng_error)?,
        assigned: row.get::<_, i64>(16).map_err(rusqlite_to_eng_error)? != 0,
        created_at: row.get(17).map_err(rusqlite_to_eng_error)?,
        updated_at: row.get(18).map_err(rusqlite_to_eng_error)?,
        user_id: 1,
    })
}

/// Create a new Chiasm task in the database.
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
    let expected_output = req.expected_output.clone();
    let output_format = req
        .output_format
        .clone()
        .unwrap_or_else(|| "raw".to_string());
    let condition = req.condition.clone();
    let guardrail_url = req.guardrail_url.clone();
    let heartbeat_interval = req.heartbeat_interval.unwrap_or(300);
    let status_ins = status.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO chiasm_tasks (agent, project, title, status, summary, \
                 expected_output, output_format, condition, guardrail_url, heartbeat_interval) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    agent,
                    project,
                    title,
                    status_ins,
                    summary,
                    expected_output,
                    output_format,
                    condition,
                    guardrail_url,
                    heartbeat_interval
                ],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await?;
    let task = get_task(db, id, user_id).await?;
    super::emit_chiasm_event(
        db,
        "task.created",
        serde_json::json!({
            "task_id": task.id,
            "agent": task.agent,
            "project": task.project,
            "title": task.title,
        }),
    )
    .await;
    Ok(task)
}

/// Retrieve a single task by ID.
#[tracing::instrument(skip(db), fields(task_id = id, user_id))]
pub async fn get_task(db: &Database, id: i64, _user_id: i64) -> Result<Task> {
    let sql = format!("SELECT {TASK_COLUMNS} FROM chiasm_tasks WHERE id = ?1");

    db.read(move |conn| {
        let mut stmt = conn.prepare(&sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;
        let row = rows
            .next()
            .map_err(rusqlite_to_eng_error)?
            .ok_or_else(|| EngError::NotFound(format!("task {}", id)))?;
        row_to_task(row)
    })
    .await
}

/// List tasks with optional filtering by status, agent, and project.
#[tracing::instrument(skip(db), fields(user_id, status = ?status, agent = ?agent, project = ?project, limit, offset))]
pub async fn list_tasks(
    db: &Database,
    _user_id: i64,
    status: Option<&str>,
    agent: Option<&str>,
    project: Option<&str>,
    limit: usize,
    offset: usize,
) -> Result<Vec<Task>> {
    let mut sql = format!("SELECT {TASK_COLUMNS} FROM chiasm_tasks");
    let mut clauses: Vec<String> = Vec::new();
    let mut idx = 1usize;
    let mut params: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(s) = status {
        clauses.push(format!("status = ?{}", idx));
        params.push(rusqlite::types::Value::Text(s.to_string()));
        idx += 1;
    }
    if let Some(a) = agent {
        clauses.push(format!("agent = ?{}", idx));
        params.push(rusqlite::types::Value::Text(a.to_string()));
        idx += 1;
    }
    if let Some(p) = project {
        clauses.push(format!("project = ?{}", idx));
        params.push(rusqlite::types::Value::Text(p.to_string()));
        idx += 1;
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
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
#[tracing::instrument(skip(db, req), fields(task_id = id, user_id))]
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
                "SELECT agent, status, summary FROM chiasm_tasks WHERE id = ?1",
                rusqlite::params![id],
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

        let sql = format!("UPDATE chiasm_tasks SET {} WHERE id = ?", sets.join(", "));
        params_dyn.push(Box::new(id));
        let refs: Vec<&dyn rusqlite::ToSql> = params_dyn.iter().map(|b| b.as_ref()).collect();
        tx.execute(&sql, refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;

        tx.execute(
            "INSERT INTO chiasm_task_updates (task_id, agent, status, summary)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![id, new_agent, new_status, new_summary],
        )
        .map_err(rusqlite_to_eng_error)?;

        Ok(())
    })
    .await?;
    let task = get_task(db, id, user_id).await?;
    let event_action = if task.status == "completed" {
        "task.completed"
    } else {
        "task.updated"
    };
    super::emit_chiasm_event(
        db,
        event_action,
        serde_json::json!({
            "task_id": task.id,
            "status": task.status,
            "agent": task.agent,
        }),
    )
    .await;
    Ok(task)
}

/// Delete a task by ID.
#[tracing::instrument(skip(db), fields(task_id = id))]
pub async fn delete_task(db: &Database, id: i64) -> Result<()> {
    db.write(move |conn| {
        conn.execute(
            "DELETE FROM chiasm_tasks WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

/// Return the update history for a task in reverse chronological order.
#[tracing::instrument(skip(db), fields(task_id, user_id, limit))]
pub async fn list_task_history(
    db: &Database,
    task_id: i64,
    user_id: i64,
    limit: usize,
) -> Result<Vec<TaskUpdate>> {
    let sql = "SELECT id, task_id, agent, status, summary, created_at
               FROM chiasm_task_updates
               WHERE task_id = ?1
               ORDER BY id DESC
               LIMIT ?2";

    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![task_id, limit as i64])
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
                user_id,
            });
        }
        Ok(out)
    })
    .await
}

/// Return aggregated task counts grouped by status.
#[tracing::instrument(skip(db))]
pub async fn get_stats(db: &Database) -> Result<ChiasmStats> {
    db.read(move |conn| {
        let mut by_status = BTreeMap::new();
        let mut total: i64 = 0;
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
        Ok(ChiasmStats { total, by_status })
    })
    .await
}

/// Return a recent activity feed of tasks ordered by last modification time.
#[tracing::instrument(skip(db), fields(limit, offset))]
pub async fn get_feed(db: &Database, limit: usize, offset: usize) -> Result<Vec<FeedItem>> {
    let sql = "SELECT id, agent, project, title, status, summary, updated_at, created_at
               FROM chiasm_tasks
               ORDER BY updated_at DESC, id DESC
               LIMIT ?1 OFFSET ?2";

    db.read(move |conn| {
        let mut stmt = conn.prepare(sql).map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![limit as i64, offset as i64])
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

/// Submit output for a task. Stores the output string and updates the timestamp.
#[tracing::instrument(skip(db), fields(task_id = id, user_id))]
pub async fn submit_output(db: &Database, id: i64, output: &str, user_id: i64) -> Result<Task> {
    let output_s = output.to_string();
    let changed = db
        .write(move |conn| {
            conn.execute(
                "UPDATE chiasm_tasks SET output = ?1, updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![output_s, id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    if changed == 0 {
        return Err(EngError::NotFound(format!("task {}", id)));
    }
    let task = get_task(db, id, user_id).await?;
    super::emit_chiasm_event(db, "task.output", serde_json::json!({"task_id": id})).await;
    Ok(task)
}

/// Submit feedback for a task. Stores feedback and resets status to "active"
/// so the assigned agent can retry with the reviewer's guidance.
#[tracing::instrument(skip(db), fields(task_id = id, user_id))]
pub async fn submit_feedback(db: &Database, id: i64, feedback: &str, user_id: i64) -> Result<Task> {
    let feedback_s = feedback.to_string();
    let changed = db
        .write(move |conn| {
            conn.execute(
                "UPDATE chiasm_tasks SET feedback = ?1, status = 'active', \
                 updated_at = datetime('now') WHERE id = ?2",
                rusqlite::params![feedback_s, id],
            )
            .map_err(rusqlite_to_eng_error)
        })
        .await?;
    if changed == 0 {
        return Err(EngError::NotFound(format!("task {}", id)));
    }
    let task = get_task(db, id, user_id).await?;
    super::emit_chiasm_event(db, "task.feedback", serde_json::json!({"task_id": id})).await;
    Ok(task)
}

/// Unit tests.
#[cfg(test)]
mod tests {
    use super::*;

    /// Initialize an in-memory database for testing.
    async fn setup() -> Database {
        Database::connect_memory().await.expect("db")
    }

    /// Helper to build a minimal CreateTaskRequest for tests.
    fn test_req(title: &str) -> CreateTaskRequest {
        CreateTaskRequest {
            agent: "a".into(),
            project: "p".into(),
            title: title.into(),
            status: None,
            summary: None,
            user_id: Some(1),
            expected_output: None,
            output_format: None,
            condition: None,
            guardrail_url: None,
            heartbeat_interval: None,
        }
    }

    /// Test: submitting output stores the value and returns the updated task.
    #[tokio::test]
    async fn submit_output_stores_and_returns() {
        let db = setup().await;
        let t = create_task(&db, test_req("output-test")).await.unwrap();
        let updated = submit_output(&db, t.id, "result data", 1).await.unwrap();
        assert_eq!(updated.output.as_deref(), Some("result data"));
    }

    /// Test: submitting feedback stores the message and resets task status to active.
    #[tokio::test]
    async fn submit_feedback_resets_to_active() {
        let db = setup().await;
        let t = create_task(
            &db,
            CreateTaskRequest {
                status: Some("completed".into()),
                ..test_req("feedback-test")
            },
        )
        .await
        .unwrap();
        let updated = submit_feedback(&db, t.id, "needs revision", 1)
            .await
            .unwrap();
        assert_eq!(updated.feedback.as_deref(), Some("needs revision"));
        assert_eq!(updated.status, "active");
    }

    /// Test: a created task can be retrieved by ID with its fields intact.
    #[tokio::test]
    async fn create_and_get_task() {
        let db = setup().await;
        let t = create_task(
            &db,
            CreateTaskRequest {
                agent: "claude-code".into(),
                project: "kleos".into(),
                title: "port syntheos".into(),
                status: Some("active".into()),
                summary: Some("phase 27b".into()),
                user_id: Some(1),
                expected_output: None,
                output_format: None,
                condition: None,
                guardrail_url: None,
                heartbeat_interval: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(t.status, "active");
        let fetched = get_task(&db, t.id, 1).await.unwrap();
        assert_eq!(fetched.title, "port syntheos");
    }

    /// Test: updating a task writes a history entry with the new status and summary.
    #[tokio::test]
    async fn update_task_writes_history() {
        let db = setup().await;
        let t = create_task(
            &db,
            CreateTaskRequest {
                agent: "claude-code".into(),
                project: "kleos".into(),
                title: "t".into(),
                status: None,
                summary: None,
                user_id: Some(1),
                expected_output: None,
                output_format: None,
                condition: None,
                guardrail_url: None,
                heartbeat_interval: None,
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

    /// Phase 5.4 dropped user_id from chiasm tables: tenant isolation is
    /// now enforced at the database level (one shard per tenant), so a
    /// shared in-memory DB no longer separates user 1 and user 2.
    ///
    /// The shard-level invariant is now covered by:
    ///   kleos-lib/tests/tenant_isolation.rs::chiasm_tasks_isolated_across_tenants
    #[tokio::test]
    #[ignore]
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
                expected_output: None,
                output_format: None,
                condition: None,
                guardrail_url: None,
                heartbeat_interval: None,
            },
        )
        .await
        .unwrap();
        let other = list_tasks(&db, 2, None, None, None, 10, 0).await.unwrap();
        assert!(other.is_empty());
    }

    /// Test: creating a task with an unrecognised status string returns an error.
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
                expected_output: None,
                output_format: None,
                condition: None,
                guardrail_url: None,
                heartbeat_interval: None,
            },
        )
        .await;
        assert!(r.is_err());
    }

    /// Test: extended fields like expected_output and heartbeat_interval are persisted correctly.
    #[tokio::test]
    async fn create_task_with_extended_fields() {
        let db = setup().await;
        let t = create_task(
            &db,
            CreateTaskRequest {
                agent: "claude-code".into(),
                project: "kleos".into(),
                title: "port syntheos".into(),
                status: Some("active".into()),
                summary: Some("phase 52".into()),
                user_id: Some(1),
                expected_output: Some("all tests pass".into()),
                output_format: Some("json".into()),
                condition: Some("ci green".into()),
                guardrail_url: None,
                heartbeat_interval: Some(120),
            },
        )
        .await
        .unwrap();
        assert_eq!(t.expected_output.as_deref(), Some("all tests pass"));
        assert_eq!(t.output_format.as_deref(), Some("json"));
        assert_eq!(t.heartbeat_interval, 120);
        assert!(t.assigned); // default is 1/true
    }

    /// Test: non-standard status values like blocked_on_human and stale are accepted.
    #[tokio::test]
    async fn extended_statuses_accepted() {
        let db = setup().await;
        for status in &["blocked_on_human", "stale", "queued"] {
            let t = create_task(
                &db,
                CreateTaskRequest {
                    agent: "a".into(),
                    project: "p".into(),
                    title: format!("test-{}", status),
                    status: Some(status.to_string()),
                    summary: None,
                    user_id: Some(1),
                    expected_output: None,
                    output_format: None,
                    condition: None,
                    guardrail_url: None,
                    heartbeat_interval: None,
                },
            )
            .await
            .unwrap();
            assert_eq!(t.status, *status);
        }
    }
}
