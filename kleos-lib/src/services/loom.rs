use crate::db::Database;
use crate::services::axon::publish_internal;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

// --- Step definition (stored in workflow.steps JSON array) ---

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
    "action",
    "decision",
    "parallel",
    "wait",
    "webhook",
    "llm",
    "transform",
];

// --- Workflow ---

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

/// Request payload for creating a workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWorkflowRequest {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<StepDef>,
    pub user_id: Option<i64>,
}

/// Request payload for updating a workflow definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateWorkflowRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub steps: Option<Vec<StepDef>>,
}

// --- Run ---

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

/// Request payload for starting a new workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRunRequest {
    pub workflow_id: i64,
    pub workflow_name: Option<String>,
    pub input: Option<serde_json::Value>,
    pub user_id: Option<i64>,
}

// --- Step ---

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

// --- Log entry ---

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

// --- Stats ---

/// Per-category count breakdown used inside stats responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatBreakdown {
    pub name: String,
    pub count: i64,
}

/// Aggregate statistics for the Loom subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoomStats {
    pub workflows: i64,
    pub runs: i64,
    pub active_runs: i64,
    pub steps: i64,
    #[serde(default)]
    pub runs_by_status: Vec<StatBreakdown>,
}

// --- Error helper ---

// --- Row mapping helpers ---

fn row_to_workflow(row: &rusqlite::Row<'_>) -> Result<Workflow> {
    let steps_str: String = row.get(3)?;
    let steps: serde_json::Value = serde_json::from_str(&steps_str)?;
    Ok(Workflow {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        steps,
        user_id: 1,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// Map a SQLite row to a Run struct.
fn row_to_run(row: &rusqlite::Row<'_>) -> Result<Run> {
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
        user_id: 1,
        started_at: row.get(6)?,
        completed_at: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

/// Map a SQLite row to a Step struct.
fn row_to_step(row: &rusqlite::Row<'_>) -> Result<Step> {
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

/// Map a SQLite row to a LogEntry struct.
fn row_to_log_entry(row: &rusqlite::Row<'_>) -> Result<LogEntry> {
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

// --- Utility functions ---

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

// --- Logging ---

#[tracing::instrument(skip(db, message, data), fields(level = %level))]
pub async fn add_log(
    db: &Database,
    run_id: i64,
    step_id: Option<i64>,
    level: &str,
    message: &str,
    data: Option<serde_json::Value>,
) -> Result<()> {
    let data_str =
        serde_json::to_string(&data.unwrap_or(serde_json::Value::Object(serde_json::Map::new())))?;
    let level = level.to_string();
    let message = message.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO loom_run_logs (run_id, step_id, level, message, data)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![run_id, step_id, level, message, data_str],
        )?;
        Ok(())
    })
    .await
}

/// Retrieve log entries for a workflow run, with optional step and level filters.
#[tracing::instrument(skip(db))]
pub async fn get_logs(
    db: &Database,
    run_id: i64,
    step_id: Option<i64>,
    level: Option<&str>,
    limit: usize,
    user_id: i64,
) -> Result<Vec<LogEntry>> {
    // Verify run ownership
    get_run(db, run_id).await?;

    let level = level.map(|s| s.to_string());
    db.read(move |conn| {
        let limit_i64 = limit as i64;
        let mut stmt;
        let mut rows;

        if let Some(sid) = step_id {
            if let Some(ref lvl) = level {
                stmt = conn.prepare(
                    "SELECT id, run_id, step_id, level, message, data, created_at
                         FROM loom_run_logs
                         WHERE run_id = ?1 AND step_id = ?2 AND level = ?3
                         ORDER BY id ASC LIMIT ?4",
                )?;
                rows = stmt.query(rusqlite::params![run_id, sid, lvl, limit_i64])?;
            } else {
                stmt = conn.prepare(
                    "SELECT id, run_id, step_id, level, message, data, created_at
                         FROM loom_run_logs
                         WHERE run_id = ?1 AND step_id = ?2
                         ORDER BY id ASC LIMIT ?3",
                )?;
                rows = stmt.query(rusqlite::params![run_id, sid, limit_i64])?;
            }
        } else if let Some(ref lvl) = level {
            stmt = conn.prepare(
                "SELECT id, run_id, step_id, level, message, data, created_at
                     FROM loom_run_logs
                     WHERE run_id = ?1 AND level = ?2
                     ORDER BY id ASC LIMIT ?3",
            )?;
            rows = stmt.query(rusqlite::params![run_id, lvl, limit_i64])?;
        } else {
            stmt = conn.prepare(
                "SELECT id, run_id, step_id, level, message, data, created_at
                     FROM loom_run_logs
                     WHERE run_id = ?1
                     ORDER BY id ASC LIMIT ?2",
            )?;
            rows = stmt.query(rusqlite::params![run_id, limit_i64])?;
        }

        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            entries.push(row_to_log_entry(row)?);
        }
        Ok(entries)
    })
    .await
}

// --- Workflow CRUD ---

#[tracing::instrument(skip(db, req), fields(name = %req.name))]
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

    let steps_json = serde_json::to_string(&req.steps)?;
    let name = req.name.clone();
    let description = req.description.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO loom_workflows (name, description, steps)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![name, description, steps_json],
        )?;

        let mut stmt = conn.prepare(
            "SELECT id, name, description, steps, created_at, updated_at
                 FROM loom_workflows
                 WHERE rowid = last_insert_rowid()",
        )?;
        let mut rows = stmt.query(())?;

        if let Some(row) = rows.next()? {
            let wf = row_to_workflow(row)?;
            info!("created workflow '{}' id={}", wf.name, wf.id);
            Ok(wf)
        } else {
            Err(EngError::Internal(
                "failed to fetch created workflow".into(),
            ))
        }
    })
    .await
}

/// Fetch a single workflow by ID.
#[tracing::instrument(skip(db))]
pub async fn get_workflow(db: &Database, id: i64) -> Result<Workflow> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, steps, created_at, updated_at
                 FROM loom_workflows
                 WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;

        if let Some(row) = rows.next()? {
            Ok(row_to_workflow(row)?)
        } else {
            Err(EngError::NotFound(format!("workflow {}", id)))
        }
    })
    .await
}

/// Fetch a workflow by its unique name.
#[tracing::instrument(skip(db), fields(name = %name))]
pub async fn get_workflow_by_name(db: &Database, name: &str) -> Result<Workflow> {
    let name = name.to_string();
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, steps, created_at, updated_at
                 FROM loom_workflows
                 WHERE name = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![name])?;

        if let Some(row) = rows.next()? {
            Ok(row_to_workflow(row)?)
        } else {
            Err(EngError::NotFound(format!("workflow '{}'", name)))
        }
    })
    .await
}

/// List all workflow definitions.
#[tracing::instrument(skip(db))]
pub async fn list_workflows(db: &Database) -> Result<Vec<Workflow>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, name, description, steps, created_at, updated_at
                 FROM loom_workflows
                 ORDER BY name ASC",
        )?;
        let mut rows = stmt.query(())?;

        let mut workflows = Vec::new();
        while let Some(row) = rows.next()? {
            workflows.push(row_to_workflow(row)?);
        }
        Ok(workflows)
    })
    .await
}

/// Update mutable fields on an existing workflow.
#[tracing::instrument(skip(db, req))]
pub async fn update_workflow(
    db: &Database,
    id: i64,
    req: UpdateWorkflowRequest,
) -> Result<Workflow> {
    // Verify it exists
    get_workflow(db, id).await?;

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

    // Build dynamic SET -- positional params: values first, then id at end
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
        return get_workflow(db, id).await;
    }

    set_parts.push("updated_at = datetime('now')".into());

    let id_param = value_idx;
    let set_clause = set_parts.join(", ");
    let sql = format!("UPDATE loom_workflows SET {set_clause} WHERE id = ?{id_param}");

    // Serialize steps if present
    let steps_json = req.steps.as_ref().map(serde_json::to_string).transpose()?;

    let name = req.name.clone();
    let description = req.description.clone();

    db.write(move |conn| {
        // Build params as boxed ToSql values
        let mut params_dyn: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(ref n) = name {
            params_dyn.push(Box::new(n.clone()));
        }
        if let Some(ref d) = description {
            params_dyn.push(Box::new(d.clone()));
        }
        if let Some(ref sj) = steps_json {
            params_dyn.push(Box::new(sj.clone()));
        }
        params_dyn.push(Box::new(id));

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_dyn.iter().map(|b| b.as_ref()).collect();

        conn.execute(&sql, params_refs.as_slice())?;
        Ok(())
    })
    .await?;

    get_workflow(db, id).await
}

/// Delete a workflow and its associated runs and steps.
#[tracing::instrument(skip(db))]
pub async fn delete_workflow(db: &Database, id: i64) -> Result<bool> {
    db.write(move |conn| {
        let affected = conn.execute(
            "DELETE FROM loom_workflows WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(affected > 0)
    })
    .await
}

// --- Run management ---

#[tracing::instrument(skip(db, req), fields(workflow_id = req.workflow_id))]
pub async fn create_run(db: &Database, req: CreateRunRequest) -> Result<Run> {
    let _user_id = req.user_id;
    let input = req
        .input
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let input_str = serde_json::to_string(&input)?;

    // Fetch workflow and validate it has steps (tenant-scoped).
    let workflow_id = if req.workflow_id > 0 {
        req.workflow_id
    } else if let Some(ref name) = req.workflow_name {
        let wf = get_workflow_by_name(db, name).await?;
        wf.id
    } else {
        return Err(EngError::InvalidInput(
            "workflow_id or workflow_name required".into(),
        ));
    };
    let workflow = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, name, description, steps, created_at, updated_at
                     FROM loom_workflows WHERE id = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![workflow_id])?;
            if let Some(row) = rows.next()? {
                Ok(Some(row_to_workflow(row)?))
            } else {
                Ok(None)
            }
        })
        .await?
        .ok_or_else(|| EngError::NotFound(format!("workflow {}", req.workflow_id)))?;

    let step_defs: Vec<StepDef> = serde_json::from_value(workflow.steps.clone())?;
    if step_defs.is_empty() {
        return Err(EngError::InvalidInput("workflow has no steps".into()));
    }

    // Insert run and fetch it back
    let run = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO loom_runs (workflow_id, status, input, output)
                 VALUES (?1, 'pending', ?2, '{}')",
                rusqlite::params![workflow_id, input_str],
            )?;

            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, status, input, output, error,
                            started_at, completed_at, created_at, updated_at
                     FROM loom_runs WHERE rowid = last_insert_rowid()",
            )?;
            let mut rows = stmt.query(())?;

            if let Some(row) = rows.next()? {
                Ok(row_to_run(row)?)
            } else {
                Err(EngError::Internal("failed to fetch created run".into()))
            }
        })
        .await?;

    // Insert steps from workflow definition
    let run_id = run.id;
    db.write(move |conn| {
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

            conn.execute(
                "INSERT INTO loom_steps
                    (run_id, name, type, config, status, input, output,
                     depends_on, retry_count, max_retries, timeout_ms)
                 VALUES (?1, ?2, ?3, ?4, 'pending', '{}', '{}', ?5, 0, ?6, ?7)",
                rusqlite::params![
                    run_id,
                    step_def.name.clone(),
                    step_def.step_type.clone(),
                    config_str,
                    depends_on_str,
                    max_retries,
                    timeout_ms
                ],
            )?;
        }
        Ok(())
    })
    .await?;

    add_log(
        db,
        run.id,
        None,
        "info",
        &format!("run created for workflow '{}'", workflow.name),
        None,
    )
    .await?;

    let _ = publish_internal(
        db,
        "tasks",
        "loom",
        "workflow.run.created",
        serde_json::json!({
            "run_id": run.id,
            "workflow_id": run.workflow_id,
            "status": run.status,
        }),
    )
    .await;

    info!("created run {} for workflow {}", run.id, req.workflow_id);
    Ok(run)
}

/// Fetch a single run by ID.
#[tracing::instrument(skip(db), fields(run_id = id, user_id))]
pub async fn get_run(db: &Database, id: i64) -> Result<Run> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, workflow_id, status, input, output, error,
                        started_at, completed_at, created_at, updated_at
                 FROM loom_runs
                 WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;

        if let Some(row) = rows.next()? {
            Ok(row_to_run(row)?)
        } else {
            Err(EngError::NotFound(format!("run {}", id)))
        }
    })
    .await
}

/// List runs with optional status and workflow filters.
#[tracing::instrument(skip(db), fields(workflow_id = ?workflow_id, status = ?status, limit))]
pub async fn list_runs(
    db: &Database,
    workflow_id: Option<i64>,
    status: Option<&str>,
    limit: usize,
) -> Result<Vec<Run>> {
    let status = status.map(|s| s.to_string());
    db.read(move |conn| {
        let limit_i64 = limit as i64;
        let mut stmt;
        let mut rows;

        match (workflow_id, &status) {
            (Some(wid), Some(st)) => {
                stmt = conn.prepare(
                    "SELECT id, workflow_id, status, input, output, error,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         WHERE workflow_id = ?1 AND status = ?2
                         ORDER BY id DESC LIMIT ?3",
                )?;
                rows = stmt.query(rusqlite::params![wid, st, limit_i64])?;
            }
            (Some(wid), None) => {
                stmt = conn.prepare(
                    "SELECT id, workflow_id, status, input, output, error,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         WHERE workflow_id = ?1
                         ORDER BY id DESC LIMIT ?2",
                )?;
                rows = stmt.query(rusqlite::params![wid, limit_i64])?;
            }
            (None, Some(st)) => {
                stmt = conn.prepare(
                    "SELECT id, workflow_id, status, input, output, error,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         WHERE status = ?1
                         ORDER BY id DESC LIMIT ?2",
                )?;
                rows = stmt.query(rusqlite::params![st, limit_i64])?;
            }
            (None, None) => {
                stmt = conn.prepare(
                    "SELECT id, workflow_id, status, input, output, error,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         ORDER BY id DESC LIMIT ?1",
                )?;
                rows = stmt.query(rusqlite::params![limit_i64])?;
            }
        }

        let mut runs = Vec::new();
        while let Some(row) = rows.next()? {
            runs.push(row_to_run(row)?);
        }
        Ok(runs)
    })
    .await
}

/// Mark a running workflow run as cancelled.
#[tracing::instrument(skip(db), fields(run_id = id, user_id))]
pub async fn cancel_run(db: &Database, id: i64, _user_id: i64) -> Result<bool> {
    let run = get_run(db, id).await?;

    // Check not already terminal
    if matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(false);
    }

    // Mark run cancelled and skip pending/running steps
    db.write(move |conn| {
        conn.execute(
            "UPDATE loom_runs
             SET status = 'cancelled', updated_at = datetime('now')
             WHERE id = ?1",
            rusqlite::params![id],
        )?;

        conn.execute(
            "UPDATE loom_steps
             SET status = 'skipped'
             WHERE run_id = ?1 AND status IN ('pending', 'running')",
            rusqlite::params![id],
        )?;

        Ok(())
    })
    .await?;

    add_log(db, id, None, "info", "run cancelled", None).await?;

    let _ = publish_internal(
        db,
        "tasks",
        "loom",
        "workflow.run.cancelled",
        serde_json::json!({
            "run_id": id,
        }),
    )
    .await;

    info!("cancelled run {}", id);
    Ok(true)
}

// --- Step management ---

#[tracing::instrument(skip(db), fields(run_id, user_id))]
pub async fn get_steps(db: &Database, run_id: i64, _user_id: i64) -> Result<Vec<Step>> {
    // Verify run ownership
    get_run(db, run_id).await?;
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, name, type, config, status, input, output,
                        error, depends_on, retry_count, max_retries, timeout_ms,
                        started_at, completed_at, created_at
                 FROM loom_steps
                 WHERE run_id = ?1
                 ORDER BY id ASC",
        )?;
        let mut rows = stmt.query(rusqlite::params![run_id])?;

        let mut steps = Vec::new();
        while let Some(row) = rows.next()? {
            steps.push(row_to_step(row)?);
        }
        Ok(steps)
    })
    .await
}

/// Fetch a single step by ID.
#[tracing::instrument(skip(db), fields(step_id = id))]
pub async fn get_step(db: &Database, id: i64) -> Result<Step> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, run_id, name, type, config, status, input, output,
                        error, depends_on, retry_count, max_retries, timeout_ms,
                        started_at, completed_at, created_at
                 FROM loom_steps
                 WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;

        if let Some(row) = rows.next()? {
            Ok(row_to_step(row)?)
        } else {
            Err(EngError::NotFound(format!("step {}", id)))
        }
    })
    .await
}

/// Mark a step as completed and advance the run.
#[tracing::instrument(skip(db, output), fields(step_id, user_id))]
pub async fn complete_step(
    db: &Database,
    step_id: i64,
    output: serde_json::Value,
    _user_id: i64,
) -> Result<Step> {
    let step = get_step(db, step_id).await?;
    // Verify run ownership
    get_run(db, step.run_id).await?;

    if step.status != "running" {
        return Err(EngError::InvalidInput(format!(
            "cannot complete step {} -- current status is '{}'",
            step_id, step.status
        )));
    }

    let output_str = serde_json::to_string(&output)?;

    db.write(move |conn| {
        conn.execute(
            "UPDATE loom_steps
             SET status = 'completed', output = ?1,
                 completed_at = datetime('now')
             WHERE id = ?2",
            rusqlite::params![output_str, step_id],
        )?;
        Ok(())
    })
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
    get_step(db, step_id).await
}

/// Mark a step as failed and handle retry or run failure.
#[tracing::instrument(skip(db, error), fields(step_id, user_id))]
pub async fn fail_step(db: &Database, step_id: i64, error: &str, _user_id: i64) -> Result<Step> {
    let step = get_step(db, step_id).await?;
    // Verify run ownership
    get_run(db, step.run_id).await?;

    let error = error.to_string();

    if step.retry_count < step.max_retries {
        // Retry -- reset to pending with incremented count
        let err_clone = error.clone();
        db.write(move |conn| {
            conn.execute(
                "UPDATE loom_steps
                 SET status = 'pending', retry_count = retry_count + 1,
                     error = ?1, started_at = NULL
                 WHERE id = ?2",
                rusqlite::params![err_clone, step_id],
            )?;
            Ok(())
        })
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
        let err_step = error.clone();
        let err_run = error.clone();
        let run_id = step.run_id;
        db.write(move |conn| {
            conn.execute(
                "UPDATE loom_steps
                 SET status = 'failed', error = ?1,
                     completed_at = datetime('now')
                 WHERE id = ?2",
                rusqlite::params![err_step, step_id],
            )?;

            conn.execute(
                "UPDATE loom_runs
                 SET status = 'failed', error = ?1,
                     completed_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?2",
                rusqlite::params![err_run, run_id],
            )?;

            Ok(())
        })
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

        let _ = publish_internal(
            db,
            "alerts",
            "loom",
            "workflow.run.failed",
            serde_json::json!({
                "run_id": run_id,
                "error": "retries exhausted",
            }),
        )
        .await;

        warn!(
            "run {} failed: step '{}' exhausted retries",
            step.run_id, step.name
        );
    }

    get_step(db, step_id).await
}

// --- Core orchestration ---

#[tracing::instrument(skip(db), fields(run_id))]
pub async fn advance_run(db: &Database, run_id: i64) -> Result<()> {
    // Fetch run (internal function)
    let run = db
        .read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, workflow_id, status, input, output, error,
                            started_at, completed_at, created_at, updated_at
                     FROM loom_runs WHERE id = ?1",
            )?;
            let mut rows = stmt.query(rusqlite::params![run_id])?;

            if let Some(row) = rows.next()? {
                Ok(Some(row_to_run(row)?))
            } else {
                Ok(None)
            }
        })
        .await?
        .ok_or_else(|| EngError::NotFound(format!("run {}", run_id)))?;

    // Check not terminal
    if matches!(run.status.as_str(), "completed" | "failed" | "cancelled") {
        return Ok(());
    }

    // If still pending, mark as running
    if run.status == "pending" {
        db.write(move |conn| {
            conn.execute(
                "UPDATE loom_runs
                 SET status = 'running', started_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?1",
                rusqlite::params![run_id],
            )?;
            Ok(())
        })
        .await?;
    }

    let steps = get_steps(db, run_id, run.user_id).await?;

    // Build name->step map for dependency resolution
    let step_map: HashMap<String, Step> =
        steps.iter().map(|s| (s.name.clone(), s.clone())).collect();

    // Check if all steps are done (no pending or running)
    let all_done = steps
        .iter()
        .all(|s| !matches!(s.status.as_str(), "pending" | "running"));

    if all_done {
        // Merge outputs from all completed steps into a single object.
        // Later steps overwrite earlier ones for the same key (matches TS Object.assign behavior).
        let mut merged = serde_json::Map::new();
        for step in steps.iter().filter(|s| s.status == "completed") {
            if let serde_json::Value::Object(ref map) = step.output {
                for (k, v) in map {
                    merged.insert(k.clone(), v.clone());
                }
            }
        }
        let output_str = serde_json::to_string(&serde_json::Value::Object(merged))?;

        db.write(move |conn| {
            conn.execute(
                "UPDATE loom_runs
                 SET status = 'completed', output = ?1,
                     completed_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?2",
                rusqlite::params![output_str, run_id],
            )?;
            Ok(())
        })
        .await?;

        add_log(db, run_id, None, "info", "run completed", None).await?;

        let _ = publish_internal(
            db,
            "tasks",
            "loom",
            "workflow.run.completed",
            serde_json::json!({
                "run_id": run_id,
            }),
        )
        .await;

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
        timeout_ms: i32,
    }

    let mut ready_steps: Vec<ReadyStep> = Vec::new();

    for step in &steps {
        if step.status != "pending" {
            continue;
        }

        // Check if all dependencies are completed
        let deps: Vec<String> = serde_json::from_value(step.depends_on.clone()).unwrap_or_default();

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
            timeout_ms: step.timeout_ms,
        });
    }

    // Process ready steps
    for ready in ready_steps {
        let input_str = serde_json::to_string(&ready.merged_input)?;
        let ready_id = ready.id;

        db.write(move |conn| {
            conn.execute(
                "UPDATE loom_steps
                 SET status = 'running', input = ?1,
                     started_at = datetime('now')
                 WHERE id = ?2",
                rusqlite::params![input_str, ready_id],
            )?;
            Ok(())
        })
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
                execute_transform_step(
                    db,
                    ready.id,
                    &ready.config,
                    &ready.merged_input,
                    run.user_id,
                )
                .await?;
            }
            "webhook" => {
                execute_webhook_step(
                    db,
                    ready.id,
                    &ready.config,
                    &ready.merged_input,
                    ready.timeout_ms,
                    run.user_id,
                )
                .await?;
            }
            "llm" => {
                execute_llm_step(
                    db,
                    ready.id,
                    &ready.config,
                    &ready.merged_input,
                    ready.timeout_ms,
                    run.user_id,
                )
                .await?;
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

// --- Transform executor ---

#[tracing::instrument(skip(db, config, input), fields(step_id, user_id))]
pub async fn execute_transform_step(
    db: &Database,
    step_id: i64,
    config: &serde_json::Value,
    input: &serde_json::Value,
    user_id: i64,
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
            Box::pin(complete_step(db, step_id, output, user_id)).await?;
        }
        Err(err_msg) => {
            Box::pin(fail_step(db, step_id, &err_msg, user_id)).await?;
        }
    }

    Ok(())
}

// --- Stats ---

#[tracing::instrument(skip(db), fields(user_id = ?user_id))]
pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<LoomStats> {
    let (workflows, runs, active_runs, steps, runs_by_status) = if let Some(_uid) = user_id {
        db.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT
                        (SELECT COUNT(*) FROM loom_workflows),
                        (SELECT COUNT(*) FROM loom_runs),
                        (SELECT COUNT(*) FROM loom_runs WHERE status IN ('pending','running')),
                        (SELECT COUNT(*) FROM loom_steps)",
            )?;
            let mut rows = stmt.query(())?;

            let (w, ru, ar, s) = if let Some(row) = rows.next()? {
                let w: i64 = row.get(0)?;
                let ru: i64 = row.get(1)?;
                let ar: i64 = row.get(2)?;
                let s: i64 = row.get(3)?;
                (w, ru, ar, s)
            } else {
                (0i64, 0i64, 0i64, 0i64)
            };

            let mut runs_by_status = Vec::new();
            let mut stmt = conn.prepare(
                "SELECT status, COUNT(*) as cnt FROM loom_runs \
                     GROUP BY status ORDER BY cnt DESC",
            )?;
            let mut rows = stmt.query(())?;
            while let Some(r) = rows.next()? {
                runs_by_status.push(StatBreakdown {
                    name: r.get(0)?,
                    count: r.get(1)?,
                });
            }

            Ok((w, ru, ar, s, runs_by_status))
        })
        .await?
    } else {
        db.read(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT
                        (SELECT COUNT(*) FROM loom_workflows),
                        (SELECT COUNT(*) FROM loom_runs),
                        (SELECT COUNT(*) FROM loom_runs WHERE status IN ('pending','running')),
                        (SELECT COUNT(*) FROM loom_steps)",
            )?;
            let mut rows = stmt.query(())?;

            let (w, ru, ar, s) = if let Some(row) = rows.next()? {
                let w: i64 = row.get(0)?;
                let ru: i64 = row.get(1)?;
                let ar: i64 = row.get(2)?;
                let s: i64 = row.get(3)?;
                (w, ru, ar, s)
            } else {
                (0i64, 0i64, 0i64, 0i64)
            };

            let mut runs_by_status = Vec::new();
            let mut stmt = conn.prepare(
                "SELECT status, COUNT(*) as cnt FROM loom_runs \
                     GROUP BY status ORDER BY cnt DESC",
            )?;
            let mut rows = stmt.query(())?;
            while let Some(r) = rows.next()? {
                runs_by_status.push(StatBreakdown {
                    name: r.get(0)?,
                    count: r.get(1)?,
                });
            }

            Ok((w, ru, ar, s, runs_by_status))
        })
        .await?
    };

    Ok(LoomStats {
        workflows,
        runs,
        active_runs,
        steps,
        runs_by_status,
    })
}

// --- Webhook + LLM executors (ports of the standalone loom advance_run executors) ---

/// Shared HTTP client for webhook + LLM step execution. Allocated once per
/// process; each call layers its own per-step timeout via `RequestBuilder::timeout`.
static LOOM_HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    crate::net::safe_client_builder()
        .pool_max_idle_per_host(4)
        .build()
        .expect("LOOM_HTTP_CLIENT build failed")
});

/// Maximum body slice retained in error messages from non-2xx responses, in
/// bytes. Mirrors the standalone's `text.slice(0, 500)`.
const LOOM_ERR_BODY_CAP: usize = 500;

/// SSRF guard for a Loom step's user-supplied URL. Resolves the host via DNS
/// and rejects loopback / RFC1918 / link-local / cloud-metadata targets,
/// including hostnames that resolve to such IPs -- closing the gap where the
/// synchronous `validate_webhook_url` only inspects literal IPs/hostnames.
///
/// The validated IP is deliberately NOT pinned into the URL: Loom steps
/// routinely target https domains (LLM providers, SaaS webhooks), and rewriting
/// the host to a literal IP would break TLS SNI / certificate validation. The
/// shared `LOOM_HTTP_CLIENT` (built via `net::safe_client_builder`) still
/// revalidates every redirect hop, so a redirect to a private IP is rejected.
async fn guard_step_url_ssrf(url: &str) -> Result<()> {
    crate::webhooks::resolve_and_validate_url(url)
        .await
        .map(|_| ())
}

/// Execute a `webhook`-type step by POSTing `{ step_id, run_id, input, config }`
/// to the configured URL, then completing the step with the parsed JSON
/// response. Non-2xx responses and transport errors route through `fail_step`
/// so the existing retry machinery applies.
#[tracing::instrument(skip(db, config, input), fields(step_id, user_id, timeout_ms))]
pub async fn execute_webhook_step(
    db: &Database,
    step_id: i64,
    config: &serde_json::Value,
    input: &serde_json::Value,
    timeout_ms: i32,
    user_id: i64,
) -> Result<()> {
    let url = match config.get("url").and_then(|v| v.as_str()) {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => {
            Box::pin(fail_step(
                db,
                step_id,
                "webhook step requires config.url",
                user_id,
            ))
            .await?;
            return Ok(());
        }
    };

    // SECURITY (SSRF): resolve + validate at request time so a hostname that
    // resolves to a private/metadata IP is rejected, not just literal private
    // IPs (the old validate_webhook_url check).
    if let Err(e) = guard_step_url_ssrf(&url).await {
        let msg = format!("webhook url rejected: {}", e);
        Box::pin(fail_step(db, step_id, &msg, user_id)).await?;
        return Ok(());
    }

    let method = config
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("POST")
        .to_uppercase();
    let timeout = std::time::Duration::from_millis(timeout_ms.max(1) as u64);

    let body = serde_json::json!({
        "step_id": step_id,
        "input": input,
        "config": config,
    });

    let mut req = match method.as_str() {
        "GET" => LOOM_HTTP_CLIENT.get(&url),
        "PUT" => LOOM_HTTP_CLIENT.put(&url).json(&body),
        "DELETE" => LOOM_HTTP_CLIENT.delete(&url),
        "PATCH" => LOOM_HTTP_CLIENT.patch(&url).json(&body),
        _ => LOOM_HTTP_CLIENT.post(&url).json(&body),
    };

    // Layer per-step timeout and optional caller-supplied headers.
    req = req.timeout(timeout);
    if let Some(serde_json::Value::Object(headers)) = config.get("headers") {
        for (k, v) in headers {
            if let Some(val) = v.as_str() {
                req = req.header(k.as_str(), val);
            }
        }
    }

    let response = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("webhook request failed: {}", e);
            Box::pin(fail_step(db, step_id, &msg, user_id)).await?;
            return Ok(());
        }
    };

    let status = response.status();
    let text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        let snippet: String = text.chars().take(LOOM_ERR_BODY_CAP).collect();
        let msg = format!("HTTP {}: {}", status.as_u16(), snippet);
        Box::pin(fail_step(db, step_id, &msg, user_id)).await?;
        return Ok(());
    }

    let output: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({ "body": text }));
    Box::pin(complete_step(db, step_id, output, user_id)).await?;
    Ok(())
}

/// Execute an `llm`-type step by calling the configured endpoint. Detects
/// OpenAI-compatible chat completions (`/v1/chat`, `/chat/completions`) versus
/// the engram-style `{system, prompt}` shape. When `config.schema` is set, the
/// system prompt is augmented with JSON-only instructions and the response is
/// validated as JSON with up to 3 retries before failing the step.
///
/// Supported config keys:
/// - `url` (required)
/// - `api_key`, `model`, `temperature`
/// - `system`, `prompt` -- both support `{{var}}` interpolation against the
///   merged step input
/// - `input_map` -- map of `{ alias: source_key }` to rename input variables
/// - `schema` -- JSON object embedded into the system prompt for structured output
#[tracing::instrument(skip(db, config, input), fields(step_id, user_id, timeout_ms))]
pub async fn execute_llm_step(
    db: &Database,
    step_id: i64,
    config: &serde_json::Value,
    input: &serde_json::Value,
    timeout_ms: i32,
    user_id: i64,
) -> Result<()> {
    let url = match config.get("url").and_then(|v| v.as_str()) {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => {
            Box::pin(fail_step(
                db,
                step_id,
                "llm step requires config.url",
                user_id,
            ))
            .await?;
            return Ok(());
        }
    };

    // SECURITY (SSRF): the llm step POSTs the configured api_key to this URL, so
    // resolve + validate it (rejecting hosts that resolve to private/metadata
    // IPs) before any request. Previously the llm step did no URL validation.
    if let Err(e) = guard_step_url_ssrf(&url).await {
        let msg = format!("llm url rejected: {}", e);
        Box::pin(fail_step(db, step_id, &msg, user_id)).await?;
        return Ok(());
    }

    let vars = serde_json::Value::Object(build_llm_vars(input, config.get("input_map")));

    let prompt_template = config.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    let user_prompt = interpolate(prompt_template, &vars);

    let system_template = config.get("system").and_then(|v| v.as_str());
    let mut system_prompt = match system_template {
        Some(s) => interpolate(s, &vars),
        None => "You are a helpful assistant.".to_string(),
    };

    let schema = config.get("schema").cloned();
    if let Some(ref s) = schema {
        let schema_pretty = serde_json::to_string_pretty(s).unwrap_or_else(|_| s.to_string());
        system_prompt.push_str(&format!(
            "\n\nYou MUST respond with a valid JSON object matching this schema:\n{}\n\nRespond with ONLY the JSON object, no other text.",
            schema_pretty
        ));
    }

    let model = config
        .get("model")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| "gpt-4o-mini".to_string());
    let api_key = config
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let temperature = config
        .get("temperature")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.7);
    let timeout = std::time::Duration::from_millis(timeout_ms.max(1) as u64);

    let is_openai_compat = url.contains("/v1/chat") || url.contains("/chat/completions");

    let body = if is_openai_compat {
        serde_json::json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_prompt },
            ],
            "temperature": temperature,
        })
    } else {
        serde_json::json!({
            "system": system_prompt,
            "prompt": user_prompt,
            "model": model,
        })
    };

    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err: String = String::new();

    for attempt in 1..=MAX_ATTEMPTS {
        let mut req = LOOM_HTTP_CLIENT.post(&url).timeout(timeout).json(&body);
        if let Some(ref key) = api_key {
            req = req.bearer_auth(key);
        }

        let response = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("llm request failed: {}", e);
                if attempt == MAX_ATTEMPTS {
                    Box::pin(fail_step(db, step_id, &last_err, user_id)).await?;
                    return Ok(());
                }
                continue;
            }
        };

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if !status.is_success() {
            let snippet: String = text.chars().take(LOOM_ERR_BODY_CAP).collect();
            last_err = format!("LLM HTTP {}: {}", status.as_u16(), snippet);
            if attempt == MAX_ATTEMPTS {
                Box::pin(fail_step(db, step_id, &last_err, user_id)).await?;
                return Ok(());
            }
            continue;
        }

        let data: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => serde_json::Value::Null,
        };

        // Extract the model's text content from whichever response shape arrived.
        let extracted = if is_openai_compat {
            data.pointer("/choices/0/message/content")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_default()
        } else {
            data.get("result")
                .or_else(|| data.get("text"))
                .or_else(|| data.get("content"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| text.clone())
        };

        if schema.is_some() {
            // Pull the first {...} block out of the model text and parse it.
            let candidate = extract_json_object(&extracted).unwrap_or(extracted.clone());
            match serde_json::from_str::<serde_json::Value>(&candidate) {
                Ok(parsed) => {
                    let output = serde_json::json!({
                        "result": parsed,
                        "raw": extracted,
                        "attempt": attempt,
                    });
                    Box::pin(complete_step(db, step_id, output, user_id)).await?;
                    return Ok(());
                }
                Err(e) => {
                    last_err = format!(
                        "LLM returned invalid JSON (attempt {}): {} -- response: {}",
                        attempt,
                        e,
                        extracted
                            .chars()
                            .take(LOOM_ERR_BODY_CAP)
                            .collect::<String>()
                    );
                    if attempt == MAX_ATTEMPTS {
                        Box::pin(fail_step(db, step_id, &last_err, user_id)).await?;
                        return Ok(());
                    }
                    continue;
                }
            }
        }

        let output = serde_json::json!({
            "result": extracted,
            "attempt": attempt,
        });
        Box::pin(complete_step(db, step_id, output, user_id)).await?;
        return Ok(());
    }

    // Defensive: only reached if MAX_ATTEMPTS is 0.
    Box::pin(fail_step(
        db,
        step_id,
        if last_err.is_empty() {
            "llm step exhausted retries"
        } else {
            last_err.as_str()
        },
        user_id,
    ))
    .await?;
    Ok(())
}

/// Build the template-variable context for an LLM step. Starts with every
/// key in the step's merged input (object only -- non-object inputs are
/// ignored), then applies the optional `input_map` aliases.
fn build_llm_vars(
    input: &serde_json::Value,
    input_map: Option<&serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut vars = serde_json::Map::new();
    if let serde_json::Value::Object(map) = input {
        for (k, v) in map {
            vars.insert(k.clone(), v.clone());
        }
    }
    if let Some(serde_json::Value::Object(aliases)) = input_map {
        for (alias, source) in aliases {
            if let Some(src) = source.as_str() {
                if let Some(val) = vars.get(src).cloned() {
                    vars.insert(alias.clone(), val);
                }
            }
        }
    }
    vars
}

/// Locate the first balanced `{...}` JSON object embedded in `text`. Returns
/// `None` when no opening brace is present or braces are unbalanced.
fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}
