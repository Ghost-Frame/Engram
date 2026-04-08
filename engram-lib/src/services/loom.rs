use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::{EngError, Result};
use std::collections::HashMap;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Step definition (stored in workflow.steps JSON array)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDef {
    pub name: String,
    #[serde(rename = "type")]
    pub step_type: String,
    pub config: Option<serde_json::Value>,
    pub depends_on: Option<Vec<String>>,
    pub max_retries: Option<i32>,
    pub timeout_ms: Option<i32>,
}

// Valid step types
const VALID_STEP_TYPES: &[&str] = &[
    "action", "decision", "parallel", "wait", "webhook", "llm", "transform",
];

// ---------------------------------------------------------------------------
// Workflow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub steps: serde_json::Value, // JSON array of StepDef
    pub user_id: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<StepDef>,
    pub user_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWorkflowRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub steps: Option<Vec<StepDef>>,
}

// ---------------------------------------------------------------------------
// Run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub workflow_id: i64,
    pub status: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub error: Option<String>,
    pub user_id: i64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    pub workflow_id: i64,
    pub input: Option<serde_json::Value>,
    pub user_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Step
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    pub id: i64,
    pub run_id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub step_type: String,
    pub config: serde_json::Value,
    pub status: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub error: Option<String>,
    pub depends_on: serde_json::Value, // JSON array of step names
    pub retry_count: i32,
    pub max_retries: i32,
    pub timeout_ms: i32,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Log entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub id: i64,
    pub run_id: i64,
    pub step_id: Option<i64>,
    pub level: String,
    pub message: String,
    pub data: serde_json::Value,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoomStats {
    pub workflows: i64,
    pub runs: i64,
    pub active_runs: i64,
    pub steps: i64,
}

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

fn row_to_workflow(row: &libsql::Row) -> Result<Workflow> {
    let steps_str: String = row.get(3)?;
    let steps: serde_json::Value = serde_json::from_str(&steps_str)?;
    Ok(Workflow {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        steps,
        user_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn row_to_run(row: &libsql::Row) -> Result<Run> {
    let input_str: String = row.get(3)?;
    let output_str: String = row.get(4)?;
    let input: serde_json::Value = serde_json::from_str(&input_str)?;
    let output: serde_json::Value = serde_json::from_str(&output_str)?;
    Ok(Run {
        id: row.get(0)?,
        workflow_id: row.get(1)?,
        status: row.get(2)?,
        input,
        output,
        error: row.get(5)?,
        user_id: row.get(6)?,
        started_at: row.get(7)?,
        completed_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn row_to_step(row: &libsql::Row) -> Result<Step> {
    let config_str: String = row.get(4)?;
    let input_str: String = row.get(6)?;
    let output_str: String = row.get(7)?;
    let depends_on_str: String = row.get(9)?;
    let config: serde_json::Value = serde_json::from_str(&config_str)?;
    let input: serde_json::Value = serde_json::from_str(&input_str)?;
    let output: serde_json::Value = serde_json::from_str(&output_str)?;
    let depends_on: serde_json::Value = serde_json::from_str(&depends_on_str)?;
    Ok(Step {
        id: row.get(0)?,
        run_id: row.get(1)?,
        name: row.get(2)?,
        step_type: row.get(3)?,
        config,
        status: row.get(5)?,
        input,
        output,
        error: row.get(8)?,
        depends_on,
        retry_count: row.get(10)?,
        max_retries: row.get(11)?,
        timeout_ms: row.get(12)?,
        started_at: row.get(13)?,
        completed_at: row.get(14)?,
        created_at: row.get(15)?,
    })
}

fn row_to_log_entry(row: &libsql::Row) -> Result<LogEntry> {
    let data_str: String = row.get(5)?;
    let data: serde_json::Value = serde_json::from_str(&data_str)?;
    Ok(LogEntry {
        id: row.get(0)?,
        run_id: row.get(1)?,
        step_id: row.get(2)?,
        level: row.get(3)?,
        message: row.get(4)?,
        data,
        created_at: row.get(6)?,
    })
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Resolve a dot-path like "foo.bar.baz" into a nested JSON value.
pub fn resolve_dot_path(obj: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut current = obj;
    for key in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                if let Some(val) = map.get(key) {
                    current = val;
                } else {
                    return serde_json::Value::Null;
                }
            }
            _ => return serde_json::Value::Null,
        }
    }
    current.clone()
}

/// Set a dot-path like "foo.bar" on a JSON object map, creating intermediate objects.
pub fn set_dot_path(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    path: &str,
    value: serde_json::Value,
) {
    let parts: Vec<&str> = path.splitn(2, '.').collect();
    if parts.len() == 1 {
        obj.insert(parts[0].to_string(), value);
    } else {
        let entry = obj
            .entry(parts[0].to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(inner) = entry {
            set_dot_path(inner, parts[1], value);
        } else {
            // Overwrite non-object with a new object
            let mut inner = serde_json::Map::new();
            set_dot_path(&mut inner, parts[1], value);
            obj.insert(parts[0].to_string(), serde_json::Value::Object(inner));
        }
    }
}

/// Replace {{path}} placeholders in a template string with values from vars.
pub fn interpolate(template: &str, vars: &serde_json::Value) -> String {
    let mut result = template.to_string();
    // Simple iterative replacement -- find {{ and }}
    while let Some(start) = result.find("{{") {
        if let Some(end_offset) = result[start..].find("}}") {
            let path = result[start + 2..start + end_offset].trim().to_string();
            let val = resolve_dot_path(vars, &path);
            let replacement = match &val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            result.replace_range(start..start + end_offset + 2, &replacement);
        } else {
            break;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

pub async fn add_log(
    db: &Database,
    run_id: i64,
    step_id: Option<i64>,
    level: &str,
    message: &str,
    data: Option<serde_json::Value>,
) -> Result<()> {
    let data_str = serde_json::to_string(
        &data.unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
    )?;
    db.conn
        .execute(
            "INSERT INTO loom_run_logs (run_id, step_id, level, message, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            libsql::params![run_id, step_id, level, message, data_str],
        )
        .await?;
    Ok(())
}

pub async fn get_logs(
    db: &Database,
    run_id: i64,
    step_id: Option<i64>,
    level: Option<&str>,
    limit: usize,
) -> Result<Vec<LogEntry>> {
    let mut rows = if let Some(sid) = step_id {
        if let Some(lvl) = level {
            db.conn
                .query(
                    "SELECT id, run_id, step_id, level, message, data, created_at
                     FROM loom_run_logs
                     WHERE run_id = ?1 AND step_id = ?2 AND level = ?3
                     ORDER BY id ASC LIMIT ?4",
                    libsql::params![run_id, sid, lvl, limit as i64],
                )
                .await?
        } else {
            db.conn
                .query(
                    "SELECT id, run_id, step_id, level, message, data, created_at
                     FROM loom_run_logs
                     WHERE run_id = ?1 AND step_id = ?2
                     ORDER BY id ASC LIMIT ?3",
                    libsql::params![run_id, sid, limit as i64],
                )
                .await?
        }
    } else if let Some(lvl) = level {
        db.conn
            .query(
                "SELECT id, run_id, step_id, level, message, data, created_at
                 FROM loom_run_logs
                 WHERE run_id = ?1 AND level = ?2
                 ORDER BY id ASC LIMIT ?3",
                libsql::params![run_id, lvl, limit as i64],
            )
            .await?
    } else {
        db.conn
            .query(
                "SELECT id, run_id, step_id, level, message, data, created_at
                 FROM loom_run_logs
                 WHERE run_id = ?1
                 ORDER BY id ASC LIMIT ?2",
                libsql::params![run_id, limit as i64],
            )
            .await?
    };

    let mut entries = Vec::new();
    while let Some(row) = rows.next().await? {
        entries.push(row_to_log_entry(&row)?);
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// Workflow CRUD
// ---------------------------------------------------------------------------

pub async fn create_workflow(db: &Database, req: CreateWorkflowRequest) -> Result<Workflow> {
    // Validate step types
    for step in &req.steps {
        if !VALID_STEP_TYPES.contains(&step.step_type.as_str()) {
            return Err(EngError::InvalidInput(format!(
                "invalid step type '{}' -- valid types: {}",
                step.step_type,
                VALID_STEP_TYPES.join(", ")
            )));
        }
    }

    let user_id = req.user_id.unwrap_or(1);
    let steps_json = serde_json::to_string(&req.steps)?;

    db.conn
        .execute(
            "INSERT INTO loom_workflows (name, description, steps, user_id)
             VALUES (?1, ?2, ?3, ?4)",
            libsql::params![req.name, req.description, steps_json, user_id],
        )
        .await?;

    let mut rows = db
        .conn
        .query(
            "SELECT id, name, description, steps, user_id, created_at, updated_at
             FROM loom_workflows
             WHERE rowid = last_insert_rowid()",
            (),
        )
        .await?;

    if let Some(row) = rows.next().await? {
        let wf = row_to_workflow(&row)?;
        info!("created workflow '{}' id={}", wf.name, wf.id);
        Ok(wf)
    } else {
        Err(EngError::Internal("failed to fetch created workflow".into()))
    }
}

pub async fn get_workflow(db: &Database, id: i64, user_id: i64) -> Result<Workflow> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, name, description, steps, user_id, created_at, updated_at
             FROM loom_workflows
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        Ok(row_to_workflow(&row)?)
    } else {
        Err(EngError::NotFound(format!("workflow {}", id)))
    }
}

pub async fn get_workflow_by_name(db: &Database, name: &str, user_id: i64) -> Result<Workflow> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, name, description, steps, user_id, created_at, updated_at
             FROM loom_workflows
             WHERE name = ?1 AND user_id = ?2",
            libsql::params![name, user_id],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        Ok(row_to_workflow(&row)?)
    } else {
        Err(EngError::NotFound(format!("workflow '{}'", name)))
    }
}

pub async fn list_workflows(db: &Database, user_id: i64) -> Result<Vec<Workflow>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, name, description, steps, user_id, created_at, updated_at
             FROM loom_workflows
             WHERE user_id = ?1
             ORDER BY name ASC",
            libsql::params![user_id],
        )
        .await?;

    let mut workflows = Vec::new();
    while let Some(row) = rows.next().await? {
        workflows.push(row_to_workflow(&row)?);
    }
    Ok(workflows)
}

pub async fn update_workflow(
    db: &Database,
    id: i64,
    user_id: i64,
    req: UpdateWorkflowRequest,
) -> Result<Workflow> {
    // Verify it exists
    get_workflow(db, id, user_id).await?;

    if let Some(ref steps) = req.steps {
        for step in steps {
            if !VALID_STEP_TYPES.contains(&step.step_type.as_str()) {
                return Err(EngError::InvalidInput(format!(
                    "invalid step type '{}' -- valid types: {}",
                    step.step_type,
                    VALID_STEP_TYPES.join(", ")
                )));
            }
        }
    }

    // Build dynamic SET -- positional params: values first, then id and user_id at end
    let mut set_parts: Vec<String> = Vec::new();
    let mut value_idx = 1usize;

    if req.name.is_some() {
        set_parts.push(format!("name = ?{}", value_idx));
        value_idx += 1;
    }
    if req.description.is_some() {
        set_parts.push(format!("description = ?{}", value_idx));
        value_idx += 1;
    }
    if req.steps.is_some() {
        set_parts.push(format!("steps = ?{}", value_idx));
        value_idx += 1;
    }

    if set_parts.is_empty() {
        return get_workflow(db, id, user_id).await;
    }

    set_parts.push("updated_at = datetime('now')".into());

    let id_param = value_idx;
    let user_id_param = value_idx + 1;
    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE loom_workflows SET {set_clause} WHERE id = ?{id_param} AND user_id = ?{user_id_param}"
    );

    // Execute with the right combination of params
    match (&req.name, &req.description, &req.steps) {
        (Some(n), Some(d), Some(s)) => {
            let steps_json = serde_json::to_string(s)?;
            db.conn
                .execute(&sql, libsql::params![n.clone(), d.clone(), steps_json, id, user_id])
                .await?;
        }
        (Some(n), Some(d), None) => {
            db.conn
                .execute(&sql, libsql::params![n.clone(), d.clone(), id, user_id])
                .await?;
        }
        (Some(n), None, Some(s)) => {
            let steps_json = serde_json::to_string(s)?;
            db.conn
                .execute(&sql, libsql::params![n.clone(), steps_json, id, user_id])
                .await?;
        }
        (None, Some(d), Some(s)) => {
            let steps_json = serde_json::to_string(s)?;
            db.conn
                .execute(&sql, libsql::params![d.clone(), steps_json, id, user_id])
                .await?;
        }
        (Some(n), None, None) => {
            db.conn
                .execute(&sql, libsql::params![n.clone(), id, user_id])
                .await?;
        }
        (None, Some(d), None) => {
            db.conn
                .execute(&sql, libsql::params![d.clone(), id, user_id])
                .await?;
        }
        (None, None, Some(s)) => {
            let steps_json = serde_json::to_string(s)?;
            db.conn
                .execute(&sql, libsql::params![steps_json, id, user_id])
                .await?;
        }
        (None, None, None) => unreachable!(),
    }

    get_workflow(db, id, user_id).await
}

pub async fn delete_workflow(db: &Database, id: i64, user_id: i64) -> Result<bool> {
    let affected = db
        .conn
        .execute(
            "DELETE FROM loom_workflows WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;
    Ok(affected > 0)
}

// ---------------------------------------------------------------------------
// Run management
// ---------------------------------------------------------------------------

pub async fn create_run(db: &Database, req: CreateRunRequest) -> Result<Run> {
    let user_id = req.user_id.unwrap_or(1);
    let input = req
        .input
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let input_str = serde_json::to_string(&input)?;

    // Fetch workflow and validate it has steps
    let mut wf_rows = db
        .conn
        .query(
            "SELECT id, name, description, steps, user_id, created_at, updated_at
             FROM loom_workflows WHERE id = ?1",
            libsql::params![req.workflow_id],
        )
        .await?;

    let workflow = if let Some(row) = wf_rows.next().await? {
        row_to_workflow(&row)?
    } else {
        return Err(EngError::NotFound(format!("workflow {}", req.workflow_id)));
    };

    let step_defs: Vec<StepDef> = serde_json::from_value(workflow.steps.clone())?;
    if step_defs.is_empty() {
        return Err(EngError::InvalidInput("workflow has no steps".into()));
    }

    // Insert run
    db.conn
        .execute(
            "INSERT INTO loom_runs (workflow_id, status, input, output, user_id)
             VALUES (?1, 'pending', ?2, '{}', ?3)",
            libsql::params![req.workflow_id, input_str, user_id],
        )
        .await?;

    let mut run_rows = db
        .conn
        .query(
            "SELECT id, workflow_id, status, input, output, error, user_id,
                    started_at, completed_at, created_at, updated_at
             FROM loom_runs WHERE rowid = last_insert_rowid()",
            (),
        )
        .await?;

    let run = if let Some(row) = run_rows.next().await? {
        row_to_run(&row)?
    } else {
        return Err(EngError::Internal("failed to fetch created run".into()));
    };

    // Insert steps from workflow definition
    for step_def in &step_defs {
        let config_str = serde_json::to_string(
            &step_def
                .config
                .clone()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
        )?;
        let depends_on_str =
            serde_json::to_string(&step_def.depends_on.clone().unwrap_or_default())?;
        let max_retries = step_def.max_retries.unwrap_or(3);
        let timeout_ms = step_def.timeout_ms.unwrap_or(30000);

        db.conn
            .execute(
                "INSERT INTO loom_steps
                    (run_id, name, type, config, status, input, output,
                     depends_on, retry_count, max_retries, timeout_ms)
                 VALUES (?1, ?2, ?3, ?4, 'pending', '{}', '{}', ?5, 0, ?6, ?7)",
                libsql::params![
                    run.id,
                    step_def.name.clone(),
                    step_def.step_type.clone(),
                    config_str,
                    depends_on_str,
                    max_retries,
                    timeout_ms
                ],
            )
            .await?;
    }

    add_log(
        db,
        run.id,
        None,
        "info",
        &format!("run created for workflow '{}'", workflow.name),
        None,
    )
    .await?;

    info!("created run {} for workflow {}", run.id, req.workflow_id);
    Ok(run)
}

pub async fn get_run(db: &Database, id: i64, user_id: i64) -> Result<Run> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, workflow_id, status, input, output, error, user_id,
                    started_at, completed_at, created_at, updated_at
             FROM loom_runs
             WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        Ok(row_to_run(&row)?)
    } else {
        Err(EngError::NotFound(format!("run {}", id)))
    }
}

pub async fn list_runs(
    db: &Database,
    user_id: i64,
    workflow_id: Option<i64>,
    status: Option<&str>,
    limit: usize,
) -> Result<Vec<Run>> {
    let mut runs = Vec::new();

    let mut rows = match (workflow_id, status) {
        (Some(wid), Some(st)) => db
            .conn
            .query(
                "SELECT id, workflow_id, status, input, output, error, user_id,
                        started_at, completed_at, created_at, updated_at
                 FROM loom_runs
                 WHERE user_id = ?1 AND workflow_id = ?2 AND status = ?3
                 ORDER BY id DESC LIMIT ?4",
                libsql::params![user_id, wid, st, limit as i64],
            )
            .await?,
        (Some(wid), None) => db
            .conn
            .query(
                "SELECT id, workflow_id, status, input, output, error, user_id,
                        started_at, completed_at, created_at, updated_at
                 FROM loom_runs
                 WHERE user_id = ?1 AND workflow_id = ?2
                 ORDER BY id DESC LIMIT ?3",
                libsql::params![user_id, wid, limit as i64],
            )
            .await?,
        (None, Some(st)) => db
            .conn
            .query(
                "SELECT id, workflow_id, status, input, output, error, user_id,
                        started_at, completed_at, created_at, updated_at
                 FROM loom_runs
                 WHERE user_id = ?1 AND status = ?2
                 ORDER BY id DESC LIMIT ?3",
                libsql::params![user_id, st, limit as i64],
            )
            .await?,
        (None, None) => db
            .conn
            .query(
                "SELECT id, workflow_id, status, input, output, error, user_id,
                        started_at, completed_at, created_at, updated_at
                 FROM loom_runs
                 WHERE user_id = ?1
                 ORDER BY id DESC LIMIT ?2",
                libsql::params![user_id, limit as i64],
            )
            .await?,
    };

    while let Some(row) = rows.next().await? {
        runs.push(row_to_run(&row)?);
    }
    Ok(runs)
}

pub async fn cancel_run(db: &Database, id: i64, user_id: i64) -> Result<bool> {
    let run = get_run(db, id, user_id).await?;

    // Check not already terminal
    if matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(false);
    }

    // Mark run cancelled
    db.conn
        .execute(
            "UPDATE loom_runs
             SET status = 'cancelled', updated_at = datetime('now')
             WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    // Skip all pending and running steps
    db.conn
        .execute(
            "UPDATE loom_steps
             SET status = 'skipped'
             WHERE run_id = ?1 AND status IN ('pending', 'running')",
            libsql::params![id],
        )
        .await?;

    add_log(db, id, None, "info", "run cancelled", None).await?;
    info!("cancelled run {}", id);
    Ok(true)
}

// ---------------------------------------------------------------------------
// Step management
// ---------------------------------------------------------------------------

pub async fn get_steps(db: &Database, run_id: i64) -> Result<Vec<Step>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, run_id, name, type, config, status, input, output,
                    error, depends_on, retry_count, max_retries, timeout_ms,
                    started_at, completed_at, created_at
             FROM loom_steps
             WHERE run_id = ?1
             ORDER BY id ASC",
            libsql::params![run_id],
        )
        .await?;

    let mut steps = Vec::new();
    while let Some(row) = rows.next().await? {
        steps.push(row_to_step(&row)?);
    }
    Ok(steps)
}

pub async fn get_step(db: &Database, id: i64) -> Result<Step> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, run_id, name, type, config, status, input, output,
                    error, depends_on, retry_count, max_retries, timeout_ms,
                    started_at, completed_at, created_at
             FROM loom_steps
             WHERE id = ?1",
            libsql::params![id],
        )
        .await?;

    if let Some(row) = rows.next().await? {
        Ok(row_to_step(&row)?)
    } else {
        Err(EngError::NotFound(format!("step {}", id)))
    }
}

pub async fn complete_step(db: &Database, step_id: i64, output: serde_json::Value) -> Result<()> {
    let step = get_step(db, step_id).await?;

    if step.status != "running" {
        return Err(EngError::InvalidInput(format!(
            "cannot complete step {} -- current status is '{}'",
            step_id, step.status
        )));
    }

    let output_str = serde_json::to_string(&output)?;

    db.conn
        .execute(
            "UPDATE loom_steps
             SET status = 'completed', output = ?1,
                 completed_at = datetime('now')
             WHERE id = ?2",
            libsql::params![output_str, step_id],
        )
        .await?;

    add_log(
        db,
        step.run_id,
        Some(step_id),
        "info",
        &format!("step '{}' completed", step.name),
        Some(serde_json::json!({ "output": output })),
    )
    .await?;

    // Advance the run after completing this step
    advance_run(db, step.run_id).await?;
    Ok(())
}

pub async fn fail_step(db: &Database, step_id: i64, error: &str) -> Result<()> {
    let step = get_step(db, step_id).await?;

    if step.retry_count < step.max_retries {
        // Retry -- reset to pending with incremented count
        db.conn
            .execute(
                "UPDATE loom_steps
                 SET status = 'pending', retry_count = retry_count + 1,
                     error = ?1, started_at = NULL
                 WHERE id = ?2",
                libsql::params![error, step_id],
            )
            .await?;

        add_log(
            db,
            step.run_id,
            Some(step_id),
            "warn",
            &format!(
                "step '{}' failed, retrying ({}/{})",
                step.name,
                step.retry_count + 1,
                step.max_retries
            ),
            Some(serde_json::json!({ "error": error })),
        )
        .await?;

        // Re-advance so the retried step can be picked up
        advance_run(db, step.run_id).await?;
    } else {
        // Max retries exhausted -- fail step and run
        db.conn
            .execute(
                "UPDATE loom_steps
                 SET status = 'failed', error = ?1,
                     completed_at = datetime('now')
                 WHERE id = ?2",
                libsql::params![error, step_id],
            )
            .await?;

        db.conn
            .execute(
                "UPDATE loom_runs
                 SET status = 'failed', error = ?1,
                     completed_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?2",
                libsql::params![error, step.run_id],
            )
            .await?;

        add_log(
            db,
            step.run_id,
            Some(step_id),
            "error",
            &format!("step '{}' failed (max retries exhausted)", step.name),
            Some(serde_json::json!({ "error": error })),
        )
        .await?;

        warn!(
            "run {} failed: step '{}' exhausted retries",
            step.run_id, step.name
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Core orchestration
// ---------------------------------------------------------------------------

pub async fn advance_run(db: &Database, run_id: i64) -> Result<()> {
    // Fetch run without user_id check (internal function)
    let mut rows = db
        .conn
        .query(
            "SELECT id, workflow_id, status, input, output, error, user_id,
                    started_at, completed_at, created_at, updated_at
             FROM loom_runs WHERE id = ?1",
            libsql::params![run_id],
        )
        .await?;

    let run = if let Some(row) = rows.next().await? {
        row_to_run(&row)?
    } else {
        return Err(EngError::NotFound(format!("run {}", run_id)));
    };

    // Check not terminal
    if matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(());
    }

    // If still pending, mark as running
    if run.status == "pending" {
        db.conn
            .execute(
                "UPDATE loom_runs
                 SET status = 'running', started_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?1",
                libsql::params![run_id],
            )
            .await?;
    }

    let steps = get_steps(db, run_id).await?;

    // Build name->step map for dependency resolution
    let step_map: HashMap<String, Step> =
        steps.iter().map(|s| (s.name.clone(), s.clone())).collect();

    // Check if all steps are done (no pending or running)
    let all_done = steps
        .iter()
        .all(|s| !matches!(s.status.as_str(), "pending" | "running"));

    if all_done {
        // Find last completed step output to use as run output
        let last_output = steps
            .iter()
            .rfind(|s| s.status == "completed")
            .map(|s| s.output.clone())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let output_str = serde_json::to_string(&last_output)?;

        db.conn
            .execute(
                "UPDATE loom_runs
                 SET status = 'completed', output = ?1,
                     completed_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?2",
                libsql::params![output_str, run_id],
            )
            .await?;

        add_log(db, run_id, None, "info", "run completed", None).await?;
        info!("run {} completed", run_id);
        return Ok(());
    }

    // Collect ready steps (pending with all deps completed)
    struct ReadyStep {
        id: i64,
        name: String,
        step_type: String,
        config: serde_json::Value,
        merged_input: serde_json::Value,
    }

    let mut ready_steps: Vec<ReadyStep> = Vec::new();

    for step in &steps {
        if step.status != "pending" {
            continue;
        }

        // Check if all dependencies are completed
        let deps: Vec<String> = serde_json::from_value(step.depends_on.clone())
            .unwrap_or_default();

        let deps_met = deps.iter().all(|dep_name| {
            step_map
                .get(dep_name)
                .map(|dep| dep.status == "completed")
                .unwrap_or(false)
        });

        if !deps_met {
            continue;
        }

        // Build step input: run input merged with dep outputs
        let mut merged = serde_json::Map::new();

        // Start with run input
        if let serde_json::Value::Object(run_input_map) = &run.input {
            for (k, v) in run_input_map {
                merged.insert(k.clone(), v.clone());
            }
        }

        // Overlay completed dep outputs
        for dep_name in &deps {
            if let Some(dep_step) = step_map.get(dep_name) {
                if let serde_json::Value::Object(dep_out_map) = &dep_step.output {
                    for (k, v) in dep_out_map {
                        merged.insert(k.clone(), v.clone());
                    }
                }
            }
        }

        ready_steps.push(ReadyStep {
            id: step.id,
            name: step.name.clone(),
            step_type: step.step_type.clone(),
            config: step.config.clone(),
            merged_input: serde_json::Value::Object(merged),
        });
    }

    // Process ready steps
    for ready in ready_steps {
        let input_str = serde_json::to_string(&ready.merged_input)?;

        db.conn
            .execute(
                "UPDATE loom_steps
                 SET status = 'running', input = ?1,
                     started_at = datetime('now')
                 WHERE id = ?2",
                libsql::params![input_str, ready.id],
            )
            .await?;

        add_log(
            db,
            run_id,
            Some(ready.id),
            "info",
            &format!("step '{}' started", ready.name),
            None,
        )
        .await?;

        match ready.step_type.as_str() {
            "transform" => {
                // Execute inline
                execute_transform_step(db, ready.id, &ready.config, &ready.merged_input).await?;
            }
            // webhook and llm: leave as running -- Phase 2 will add HTTP executors
            "webhook" | "llm" => {
                info!(
                    "step '{}' type '{}' requires external executor (Phase 2)",
                    ready.name, ready.step_type
                );
            }
            // action, decision, parallel, wait: need external completion
            _ => {
                info!(
                    "step '{}' type '{}' waiting for external completion",
                    ready.name, ready.step_type
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Transform executor
// ---------------------------------------------------------------------------

pub async fn execute_transform_step(
    db: &Database,
    step_id: i64,
    config: &serde_json::Value,
    input: &serde_json::Value,
) -> Result<()> {
    let result: std::result::Result<serde_json::Value, String> = (|| {
        if let Some(mapping) = config.get("mapping") {
            // dot-path mapping: { "target.path": "source.path" }
            let mapping_obj = mapping
                .as_object()
                .ok_or_else(|| "mapping must be an object".to_string())?;

            let mut output = serde_json::Map::new();
            for (target_path, source_path_val) in mapping_obj {
                let source_path = source_path_val.as_str().ok_or_else(|| {
                    format!("mapping value for '{}' must be a string", target_path)
                })?;
                let value = resolve_dot_path(input, source_path);
                set_dot_path(&mut output, target_path, value);
            }
            Ok(serde_json::Value::Object(output))
        } else if let Some(template_val) = config.get("template") {
            // Template interpolation with {{var.path}} syntax
            match template_val {
                serde_json::Value::String(tmpl) => {
                    let rendered = interpolate(tmpl, input);
                    Ok(serde_json::Value::String(rendered))
                }
                serde_json::Value::Object(tmpl_obj) => {
                    // Interpolate each string value in the template object
                    let mut output = serde_json::Map::new();
                    for (k, v) in tmpl_obj {
                        let rendered = if let serde_json::Value::String(s) = v {
                            serde_json::Value::String(interpolate(s, input))
                        } else {
                            v.clone()
                        };
                        output.insert(k.clone(), rendered);
                    }
                    Ok(serde_json::Value::Object(output))
                }
                _ => Ok(input.clone()),
            }
        } else {
            // Pass-through
            Ok(input.clone())
        }
    })();

    match result {
        Ok(output) => {
            Box::pin(complete_step(db, step_id, output)).await?;
        }
        Err(err_msg) => {
            Box::pin(fail_step(db, step_id, &err_msg)).await?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<LoomStats> {
    let (workflows, runs, active_runs, steps) = if let Some(uid) = user_id {
        let mut r = db
            .conn
            .query(
                "SELECT
                    (SELECT COUNT(*) FROM loom_workflows WHERE user_id = ?1),
                    (SELECT COUNT(*) FROM loom_runs WHERE user_id = ?1),
                    (SELECT COUNT(*) FROM loom_runs WHERE user_id = ?1 AND status IN ('pending','running')),
                    (SELECT COUNT(*) FROM loom_steps
                     WHERE run_id IN (SELECT id FROM loom_runs WHERE user_id = ?1))",
                libsql::params![uid],
            )
            .await?;

        if let Some(row) = r.next().await? {
            let w: i64 = row.get(0)?;
            let ru: i64 = row.get(1)?;
            let ar: i64 = row.get(2)?;
            let s: i64 = row.get(3)?;
            (w, ru, ar, s)
        } else {
            (0, 0, 0, 0)
        }
    } else {
        let mut r = db
            .conn
            .query(
                "SELECT
                    (SELECT COUNT(*) FROM loom_workflows),
                    (SELECT COUNT(*) FROM loom_runs),
                    (SELECT COUNT(*) FROM loom_runs WHERE status IN ('pending','running')),
                    (SELECT COUNT(*) FROM loom_steps)",
                (),
            )
            .await?;

        if let Some(row) = r.next().await? {
            let w: i64 = row.get(0)?;
            let ru: i64 = row.get(1)?;
            let ar: i64 = row.get(2)?;
            let s: i64 = row.get(3)?;
            (w, ru, ar, s)
        } else {
            (0, 0, 0, 0)
        }
    };

    Ok(LoomStats {
        workflows,
        runs,
        active_runs,
        steps,
    })
}
