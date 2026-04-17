use crate::db::Database;
use crate::{EngError, Result};
use serde::{Deserialize, Serialize};
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
    "action",
    "decision",
    "parallel",
    "wait",
    "webhook",
    "llm",
    "transform",
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
// Error helper
// ---------------------------------------------------------------------------

fn rusqlite_to_eng_error(err: rusqlite::Error) -> EngError {
    EngError::DatabaseMessage(err.to_string())
}

// ---------------------------------------------------------------------------
// Row mapping helpers
// ---------------------------------------------------------------------------

fn row_to_workflow(row: &rusqlite::Row<'_>) -> Result<Workflow> {
    let steps_str: String = row.get(3).map_err(rusqlite_to_eng_error)?;
    let steps: serde_json::Value = serde_json::from_str(&steps_str)?;
    Ok(Workflow {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        name: row.get(1).map_err(rusqlite_to_eng_error)?,
        description: row.get(2).map_err(rusqlite_to_eng_error)?,
        steps,
        user_id: row.get(4).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(5).map_err(rusqlite_to_eng_error)?,
        updated_at: row.get(6).map_err(rusqlite_to_eng_error)?,
    })
}

fn row_to_run(row: &rusqlite::Row<'_>) -> Result<Run> {
    let input_str: String = row.get(3).map_err(rusqlite_to_eng_error)?;
    let output_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let input: serde_json::Value = serde_json::from_str(&input_str)?;
    let output: serde_json::Value = serde_json::from_str(&output_str)?;
    Ok(Run {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        workflow_id: row.get(1).map_err(rusqlite_to_eng_error)?,
        status: row.get(2).map_err(rusqlite_to_eng_error)?,
        input,
        output,
        error: row.get(5).map_err(rusqlite_to_eng_error)?,
        user_id: row.get(6).map_err(rusqlite_to_eng_error)?,
        started_at: row.get(7).map_err(rusqlite_to_eng_error)?,
        completed_at: row.get(8).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(9).map_err(rusqlite_to_eng_error)?,
        updated_at: row.get(10).map_err(rusqlite_to_eng_error)?,
    })
}

fn row_to_step(row: &rusqlite::Row<'_>) -> Result<Step> {
    let config_str: String = row.get(4).map_err(rusqlite_to_eng_error)?;
    let input_str: String = row.get(6).map_err(rusqlite_to_eng_error)?;
    let output_str: String = row.get(7).map_err(rusqlite_to_eng_error)?;
    let depends_on_str: String = row.get(9).map_err(rusqlite_to_eng_error)?;
    let config: serde_json::Value = serde_json::from_str(&config_str)?;
    let input: serde_json::Value = serde_json::from_str(&input_str)?;
    let output: serde_json::Value = serde_json::from_str(&output_str)?;
    let depends_on: serde_json::Value = serde_json::from_str(&depends_on_str)?;
    Ok(Step {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        run_id: row.get(1).map_err(rusqlite_to_eng_error)?,
        name: row.get(2).map_err(rusqlite_to_eng_error)?,
        step_type: row.get(3).map_err(rusqlite_to_eng_error)?,
        config,
        status: row.get(5).map_err(rusqlite_to_eng_error)?,
        input,
        output,
        error: row.get(8).map_err(rusqlite_to_eng_error)?,
        depends_on,
        retry_count: row.get(10).map_err(rusqlite_to_eng_error)?,
        max_retries: row.get(11).map_err(rusqlite_to_eng_error)?,
        timeout_ms: row.get(12).map_err(rusqlite_to_eng_error)?,
        started_at: row.get(13).map_err(rusqlite_to_eng_error)?,
        completed_at: row.get(14).map_err(rusqlite_to_eng_error)?,
        created_at: row.get(15).map_err(rusqlite_to_eng_error)?,
    })
}

fn row_to_log_entry(row: &rusqlite::Row<'_>) -> Result<LogEntry> {
    let data_str: String = row.get(5).map_err(rusqlite_to_eng_error)?;
    let data: serde_json::Value = serde_json::from_str(&data_str)?;
    Ok(LogEntry {
        id: row.get(0).map_err(rusqlite_to_eng_error)?,
        run_id: row.get(1).map_err(rusqlite_to_eng_error)?,
        step_id: row.get(2).map_err(rusqlite_to_eng_error)?,
        level: row.get(3).map_err(rusqlite_to_eng_error)?,
        message: row.get(4).map_err(rusqlite_to_eng_error)?,
        data,
        created_at: row.get(6).map_err(rusqlite_to_eng_error)?,
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
        )
        .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await
}

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
    get_run(db, run_id, user_id).await?;

    let level = level.map(|s| s.to_string());
    db.read(move |conn| {
        let limit_i64 = limit as i64;
        let mut stmt;
        let mut rows;

        if let Some(sid) = step_id {
            if let Some(ref lvl) = level {
                stmt = conn
                    .prepare(
                        "SELECT id, run_id, step_id, level, message, data, created_at
                         FROM loom_run_logs
                         WHERE run_id = ?1 AND step_id = ?2 AND level = ?3
                         ORDER BY id ASC LIMIT ?4",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                rows = stmt
                    .query(rusqlite::params![run_id, sid, lvl, limit_i64])
                    .map_err(rusqlite_to_eng_error)?;
            } else {
                stmt = conn
                    .prepare(
                        "SELECT id, run_id, step_id, level, message, data, created_at
                         FROM loom_run_logs
                         WHERE run_id = ?1 AND step_id = ?2
                         ORDER BY id ASC LIMIT ?3",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                rows = stmt
                    .query(rusqlite::params![run_id, sid, limit_i64])
                    .map_err(rusqlite_to_eng_error)?;
            }
        } else if let Some(ref lvl) = level {
            stmt = conn
                .prepare(
                    "SELECT id, run_id, step_id, level, message, data, created_at
                     FROM loom_run_logs
                     WHERE run_id = ?1 AND level = ?2
                     ORDER BY id ASC LIMIT ?3",
                )
                .map_err(rusqlite_to_eng_error)?;
            rows = stmt
                .query(rusqlite::params![run_id, lvl, limit_i64])
                .map_err(rusqlite_to_eng_error)?;
        } else {
            stmt = conn
                .prepare(
                    "SELECT id, run_id, step_id, level, message, data, created_at
                     FROM loom_run_logs
                     WHERE run_id = ?1
                     ORDER BY id ASC LIMIT ?2",
                )
                .map_err(rusqlite_to_eng_error)?;
            rows = stmt
                .query(rusqlite::params![run_id, limit_i64])
                .map_err(rusqlite_to_eng_error)?;
        }

        let mut entries = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            entries.push(row_to_log_entry(row)?);
        }
        Ok(entries)
    })
    .await
}

// ---------------------------------------------------------------------------
// Workflow CRUD
// ---------------------------------------------------------------------------

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

    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let steps_json = serde_json::to_string(&req.steps)?;
    let name = req.name.clone();
    let description = req.description.clone();

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO loom_workflows (name, description, steps, user_id)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![name, description, steps_json, user_id],
        )
        .map_err(rusqlite_to_eng_error)?;

        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, steps, user_id, created_at, updated_at
                 FROM loom_workflows
                 WHERE rowid = last_insert_rowid()",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt.query(()).map_err(rusqlite_to_eng_error)?;

        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
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

#[tracing::instrument(skip(db))]
pub async fn get_workflow(db: &Database, id: i64, user_id: i64) -> Result<Workflow> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, steps, user_id, created_at, updated_at
                 FROM loom_workflows
                 WHERE id = ?1 AND user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;

        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            Ok(row_to_workflow(row)?)
        } else {
            Err(EngError::NotFound(format!("workflow {}", id)))
        }
    })
    .await
}

#[tracing::instrument(skip(db), fields(name = %name))]
pub async fn get_workflow_by_name(db: &Database, name: &str, user_id: i64) -> Result<Workflow> {
    let name = name.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, steps, user_id, created_at, updated_at
                 FROM loom_workflows
                 WHERE name = ?1 AND user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![name, user_id])
            .map_err(rusqlite_to_eng_error)?;

        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            Ok(row_to_workflow(row)?)
        } else {
            Err(EngError::NotFound(format!("workflow '{}'", name)))
        }
    })
    .await
}

#[tracing::instrument(skip(db))]
pub async fn list_workflows(db: &Database, user_id: i64) -> Result<Vec<Workflow>> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, name, description, steps, user_id, created_at, updated_at
                 FROM loom_workflows
                 WHERE user_id = ?1
                 ORDER BY name ASC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![user_id])
            .map_err(rusqlite_to_eng_error)?;

        let mut workflows = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            workflows.push(row_to_workflow(row)?);
        }
        Ok(workflows)
    })
    .await
}

#[tracing::instrument(skip(db, req))]
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
        params_dyn.push(Box::new(user_id));

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_dyn.iter().map(|b| b.as_ref()).collect();

        conn.execute(&sql, params_refs.as_slice())
            .map_err(rusqlite_to_eng_error)?;
        Ok(())
    })
    .await?;

    get_workflow(db, id, user_id).await
}

#[tracing::instrument(skip(db))]
pub async fn delete_workflow(db: &Database, id: i64, user_id: i64) -> Result<bool> {
    db.write(move |conn| {
        let affected = conn
            .execute(
                "DELETE FROM loom_workflows WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![id, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;
        Ok(affected > 0)
    })
    .await
}

// ---------------------------------------------------------------------------
// Run management
// ---------------------------------------------------------------------------

pub async fn create_run(db: &Database, req: CreateRunRequest) -> Result<Run> {
    let user_id = req
        .user_id
        .ok_or_else(|| crate::EngError::InvalidInput("user_id required".into()))?;
    let input = req
        .input
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let input_str = serde_json::to_string(&input)?;

    // Fetch workflow and validate it has steps (tenant-scoped).
    // SECURITY: previously any caller could trigger runs against another
    // tenant's workflow by supplying its id. Filter by user_id so cross-tenant
    // workflow execution returns NotFound.
    let workflow_id = req.workflow_id;
    let workflow = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, description, steps, user_id, created_at, updated_at
                     FROM loom_workflows WHERE id = ?1 AND user_id = ?2",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![workflow_id, user_id])
                .map_err(rusqlite_to_eng_error)?;
            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
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
                "INSERT INTO loom_runs (workflow_id, status, input, output, user_id)
                 VALUES (?1, 'pending', ?2, '{}', ?3)",
                rusqlite::params![workflow_id, input_str, user_id],
            )
            .map_err(rusqlite_to_eng_error)?;

            let mut stmt = conn
                .prepare(
                    "SELECT id, workflow_id, status, input, output, error, user_id,
                            started_at, completed_at, created_at, updated_at
                     FROM loom_runs WHERE rowid = last_insert_rowid()",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt.query(()).map_err(rusqlite_to_eng_error)?;

            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
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
            )
            .map_err(rusqlite_to_eng_error)?;
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

    info!("created run {} for workflow {}", run.id, req.workflow_id);
    Ok(run)
}

pub async fn get_run(db: &Database, id: i64, user_id: i64) -> Result<Run> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, workflow_id, status, input, output, error, user_id,
                        started_at, completed_at, created_at, updated_at
                 FROM loom_runs
                 WHERE id = ?1 AND user_id = ?2",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id, user_id])
            .map_err(rusqlite_to_eng_error)?;

        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            Ok(row_to_run(row)?)
        } else {
            Err(EngError::NotFound(format!("run {}", id)))
        }
    })
    .await
}

pub async fn list_runs(
    db: &Database,
    user_id: i64,
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
                stmt = conn
                    .prepare(
                        "SELECT id, workflow_id, status, input, output, error, user_id,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         WHERE user_id = ?1 AND workflow_id = ?2 AND status = ?3
                         ORDER BY id DESC LIMIT ?4",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                rows = stmt
                    .query(rusqlite::params![user_id, wid, st, limit_i64])
                    .map_err(rusqlite_to_eng_error)?;
            }
            (Some(wid), None) => {
                stmt = conn
                    .prepare(
                        "SELECT id, workflow_id, status, input, output, error, user_id,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         WHERE user_id = ?1 AND workflow_id = ?2
                         ORDER BY id DESC LIMIT ?3",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                rows = stmt
                    .query(rusqlite::params![user_id, wid, limit_i64])
                    .map_err(rusqlite_to_eng_error)?;
            }
            (None, Some(st)) => {
                stmt = conn
                    .prepare(
                        "SELECT id, workflow_id, status, input, output, error, user_id,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         WHERE user_id = ?1 AND status = ?2
                         ORDER BY id DESC LIMIT ?3",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                rows = stmt
                    .query(rusqlite::params![user_id, st, limit_i64])
                    .map_err(rusqlite_to_eng_error)?;
            }
            (None, None) => {
                stmt = conn
                    .prepare(
                        "SELECT id, workflow_id, status, input, output, error, user_id,
                            started_at, completed_at, created_at, updated_at
                         FROM loom_runs
                         WHERE user_id = ?1
                         ORDER BY id DESC LIMIT ?2",
                    )
                    .map_err(rusqlite_to_eng_error)?;
                rows = stmt
                    .query(rusqlite::params![user_id, limit_i64])
                    .map_err(rusqlite_to_eng_error)?;
            }
        }

        let mut runs = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            runs.push(row_to_run(row)?);
        }
        Ok(runs)
    })
    .await
}

pub async fn cancel_run(db: &Database, id: i64, user_id: i64) -> Result<bool> {
    let run = get_run(db, id, user_id).await?;

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
        )
        .map_err(rusqlite_to_eng_error)?;

        conn.execute(
            "UPDATE loom_steps
             SET status = 'skipped'
             WHERE run_id = ?1 AND status IN ('pending', 'running')",
            rusqlite::params![id],
        )
        .map_err(rusqlite_to_eng_error)?;

        Ok(())
    })
    .await?;

    add_log(db, id, None, "info", "run cancelled", None).await?;
    info!("cancelled run {}", id);
    Ok(true)
}

// ---------------------------------------------------------------------------
// Step management
// ---------------------------------------------------------------------------

pub async fn get_steps(db: &Database, run_id: i64, user_id: i64) -> Result<Vec<Step>> {
    // Verify run ownership
    get_run(db, run_id, user_id).await?;
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, run_id, name, type, config, status, input, output,
                        error, depends_on, retry_count, max_retries, timeout_ms,
                        started_at, completed_at, created_at
                 FROM loom_steps
                 WHERE run_id = ?1
                 ORDER BY id ASC",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![run_id])
            .map_err(rusqlite_to_eng_error)?;

        let mut steps = Vec::new();
        while let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            steps.push(row_to_step(row)?);
        }
        Ok(steps)
    })
    .await
}

pub async fn get_step(db: &Database, id: i64) -> Result<Step> {
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, run_id, name, type, config, status, input, output,
                        error, depends_on, retry_count, max_retries, timeout_ms,
                        started_at, completed_at, created_at
                 FROM loom_steps
                 WHERE id = ?1",
            )
            .map_err(rusqlite_to_eng_error)?;
        let mut rows = stmt
            .query(rusqlite::params![id])
            .map_err(rusqlite_to_eng_error)?;

        if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
            Ok(row_to_step(row)?)
        } else {
            Err(EngError::NotFound(format!("step {}", id)))
        }
    })
    .await
}

pub async fn complete_step(
    db: &Database,
    step_id: i64,
    output: serde_json::Value,
    user_id: i64,
) -> Result<()> {
    let step = get_step(db, step_id).await?;
    // Verify run ownership
    get_run(db, step.run_id, user_id).await?;

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
        )
        .map_err(rusqlite_to_eng_error)?;
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
    Ok(())
}

pub async fn fail_step(db: &Database, step_id: i64, error: &str, user_id: i64) -> Result<()> {
    let step = get_step(db, step_id).await?;
    // Verify run ownership
    get_run(db, step.run_id, user_id).await?;

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
            )
            .map_err(rusqlite_to_eng_error)?;
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
            )
            .map_err(rusqlite_to_eng_error)?;

            conn.execute(
                "UPDATE loom_runs
                 SET status = 'failed', error = ?1,
                     completed_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?2",
                rusqlite::params![err_run, run_id],
            )
            .map_err(rusqlite_to_eng_error)?;

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
    let run = db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, workflow_id, status, input, output, error, user_id,
                            started_at, completed_at, created_at, updated_at
                     FROM loom_runs WHERE id = ?1",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![run_id])
                .map_err(rusqlite_to_eng_error)?;

            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
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
            )
            .map_err(rusqlite_to_eng_error)?;
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
        // Find last completed step output to use as run output
        let last_output = steps
            .iter()
            .rfind(|s| s.status == "completed")
            .map(|s| s.output.clone())
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let output_str = serde_json::to_string(&last_output)?;

        db.write(move |conn| {
            conn.execute(
                "UPDATE loom_runs
                 SET status = 'completed', output = ?1,
                     completed_at = datetime('now'),
                     updated_at = datetime('now')
                 WHERE id = ?2",
                rusqlite::params![output_str, run_id],
            )
            .map_err(rusqlite_to_eng_error)?;
            Ok(())
        })
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
            )
            .map_err(rusqlite_to_eng_error)?;
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
                // Execute inline
                execute_transform_step(
                    db,
                    ready.id,
                    &ready.config,
                    &ready.merged_input,
                    run.user_id,
                )
                .await?;
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

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

pub async fn get_stats(db: &Database, user_id: Option<i64>) -> Result<LoomStats> {
    let (workflows, runs, active_runs, steps) = if let Some(uid) = user_id {
        db.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT
                        (SELECT COUNT(*) FROM loom_workflows WHERE user_id = ?1),
                        (SELECT COUNT(*) FROM loom_runs WHERE user_id = ?1),
                        (SELECT COUNT(*) FROM loom_runs WHERE user_id = ?1 AND status IN ('pending','running')),
                        (SELECT COUNT(*) FROM loom_steps
                         WHERE run_id IN (SELECT id FROM loom_runs WHERE user_id = ?1))",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt
                .query(rusqlite::params![uid])
                .map_err(rusqlite_to_eng_error)?;

            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let w: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
                let ru: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
                let ar: i64 = row.get(2).map_err(rusqlite_to_eng_error)?;
                let s: i64 = row.get(3).map_err(rusqlite_to_eng_error)?;
                Ok((w, ru, ar, s))
            } else {
                Ok((0i64, 0i64, 0i64, 0i64))
            }
        })
        .await?
    } else {
        db.read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT
                        (SELECT COUNT(*) FROM loom_workflows),
                        (SELECT COUNT(*) FROM loom_runs),
                        (SELECT COUNT(*) FROM loom_runs WHERE status IN ('pending','running')),
                        (SELECT COUNT(*) FROM loom_steps)",
                )
                .map_err(rusqlite_to_eng_error)?;
            let mut rows = stmt.query(()).map_err(rusqlite_to_eng_error)?;

            if let Some(row) = rows.next().map_err(rusqlite_to_eng_error)? {
                let w: i64 = row.get(0).map_err(rusqlite_to_eng_error)?;
                let ru: i64 = row.get(1).map_err(rusqlite_to_eng_error)?;
                let ar: i64 = row.get(2).map_err(rusqlite_to_eng_error)?;
                let s: i64 = row.get(3).map_err(rusqlite_to_eng_error)?;
                Ok((w, ru, ar, s))
            } else {
                Ok((0i64, 0i64, 0i64, 0i64))
            }
        })
        .await?
    };

    Ok(LoomStats {
        workflows,
        runs,
        active_runs,
        steps,
    })
}
