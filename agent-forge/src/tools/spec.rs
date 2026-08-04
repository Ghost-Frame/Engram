//! Spec lifecycle tools: `spec_task` creates a new task spec in the forge DB;
//! `update_spec` transitions its status; `list_specs` paginates all specs;
//! `get_spec` fetches a single spec together with its linked hypotheses,
//! approaches, learnings, and verification records.

use crate::db::Database;
use crate::json_io::Output;
use crate::kleos_client::KleosClient;
use crate::spec_types::{ImplementationTask, TestProperty};
use crate::tools::{set_session_active, ToolError, ToolResult};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

/// Input for `spec_task`: all fields that define a new task specification.
/// `acceptance_criteria` requires at least 2 items; `edge_cases` requires at least 3.
#[derive(Deserialize)]
pub struct SpecTaskInput {
    /// Short statement of the work to perform.
    pub task_description: Option<String>,
    /// One of the supported task lifecycle categories.
    pub task_type: Option<String>,
    /// Observable conditions that define success.
    pub acceptance_criteria: Option<Vec<String>>,
    /// Public or internal interface behavior the work must implement.
    pub interface_contract: Option<String>,
    /// Boundary conditions the implementation must handle.
    pub edge_cases: Option<Vec<String>>,
    /// Expected source files, used by the design artifact.
    pub files_to_touch: Option<Vec<String>>,
    /// External or internal prerequisites for the work.
    pub dependencies: Option<String>,
    /// Existing behavior that must remain intact, especially for bug fixes.
    pub unchanged_behaviors: Option<Vec<String>>,
    /// Concrete work items linked to acceptance criteria.
    pub implementation_tasks: Option<Vec<ImplementationTask>>,
    /// Explicit candidates for property-based tests.
    pub test_properties: Option<Vec<TestProperty>>,
}

/// The set of recognised task types enforced at spec creation time.
const VALID_TASK_TYPES: &[&str] = &[
    "feature",
    "bugfix",
    "refactor",
    "enhancement",
    "test",
    "docs",
];

/// Reject blank values in a structured prose collection.
fn validate_non_blank(field: &str, values: &[String]) -> Result<(), ToolError> {
    if values.iter().any(|value| value.trim().is_empty()) {
        return Err(ToolError::InvalidValue(format!(
            "{field} cannot contain blank values"
        )));
    }
    Ok(())
}

/// Validate artifact-only fields and every zero-based acceptance-criterion link.
fn validate_artifact_fields(
    criteria_count: usize,
    unchanged_behaviors: &[String],
    implementation_tasks: &[ImplementationTask],
    test_properties: &[TestProperty],
) -> Result<(), ToolError> {
    validate_non_blank("unchanged_behaviors", unchanged_behaviors)?;

    for (index, task) in implementation_tasks.iter().enumerate() {
        if task.description.trim().is_empty() {
            return Err(ToolError::InvalidValue(format!(
                "implementation_tasks[{index}].description cannot be blank"
            )));
        }
        if task.criteria_indices.is_empty() {
            return Err(ToolError::InvalidValue(format!(
                "implementation_tasks[{index}] must link at least one acceptance criterion"
            )));
        }
        if let Some(criteria_index) = task
            .criteria_indices
            .iter()
            .find(|criteria_index| **criteria_index >= criteria_count)
        {
            return Err(ToolError::InvalidValue(format!(
                "implementation_tasks[{index}] references missing criterion {criteria_index}"
            )));
        }
    }

    for (index, property) in test_properties.iter().enumerate() {
        if property.description.trim().is_empty() {
            return Err(ToolError::InvalidValue(format!(
                "test_properties[{index}].description cannot be blank"
            )));
        }
        if property.criteria_index >= criteria_count {
            return Err(ToolError::InvalidValue(format!(
                "test_properties[{index}] references missing criterion {}",
                property.criteria_index
            )));
        }
    }

    Ok(())
}

/// Validate the input, persist a new spec row to the DB, set the session-active
/// marker for the enforce hook, and return the new spec ID along with any
/// skills from Kleos that are relevant to the task description.
pub fn spec_task(db: &Database, input: SpecTaskInput) -> ToolResult {
    let task_description = input
        .task_description
        .ok_or_else(|| ToolError::MissingField("task_description".into()))?;

    let task_type = input
        .task_type
        .ok_or_else(|| ToolError::MissingField("task_type".into()))?;

    if !VALID_TASK_TYPES.contains(&task_type.as_str()) {
        return Err(ToolError::InvalidValue(format!(
            "task_type must be one of: {}",
            VALID_TASK_TYPES.join(", ")
        )));
    }

    let acceptance_criteria = input
        .acceptance_criteria
        .ok_or_else(|| ToolError::MissingField("acceptance_criteria".into()))?;

    if acceptance_criteria.len() < 2 {
        return Err(ToolError::InvalidValue(
            "Minimum 2 acceptance criteria required".into(),
        ));
    }

    let interface_contract = input
        .interface_contract
        .ok_or_else(|| ToolError::MissingField("interface_contract".into()))?;

    let edge_cases = input.edge_cases.unwrap_or_default();
    if edge_cases.len() < 3 {
        return Err(ToolError::InvalidValue(
            "Minimum 3 edge cases required".into(),
        ));
    }

    let unchanged_behaviors = input.unchanged_behaviors.unwrap_or_default();
    let implementation_tasks = input.implementation_tasks.unwrap_or_default();
    let test_properties = input.test_properties.unwrap_or_default();
    validate_artifact_fields(
        acceptance_criteria.len(),
        &unchanged_behaviors,
        &implementation_tasks,
        &test_properties,
    )?;

    let id = format!("spec_{}", &Uuid::new_v4().to_string()[..8]);
    let now = Utc::now().timestamp();

    db.conn()
        .execute(
            r#"
            INSERT INTO specs (
                id, created_at, task_description, task_type,
                acceptance_criteria, interface_contract, edge_cases,
                files_to_touch, dependencies, unchanged_behaviors,
                implementation_tasks, test_properties, status
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'active')
            "#,
            rusqlite::params![
                id,
                now,
                task_description,
                task_type,
                serde_json::to_string(&acceptance_criteria).unwrap(),
                interface_contract,
                serde_json::to_string(&edge_cases).unwrap(),
                input
                    .files_to_touch
                    .map(|v| serde_json::to_string(&v).unwrap()),
                input.dependencies,
                serde_json::to_string(&unchanged_behaviors).unwrap(),
                serde_json::to_string(&implementation_tasks).unwrap(),
                serde_json::to_string(&test_properties).unwrap(),
            ],
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    set_session_active(&id, &task_type);

    // Opportunistic: search for relevant skills
    let related_skills = KleosClient::new()
        .and_then(|c| c.search_skills(&task_description, Some(5)))
        .ok()
        .and_then(|v| v.get("skills").cloned());

    let mut output = Output::ok_with_id(id, "Spec created");
    if let Some(skills) = related_skills {
        output.data = Some(serde_json::json!({ "related_skills": skills }));
    }
    Ok(output)
}

/// Input for `update_spec`: the spec to update, its new status, and an optional note.
#[derive(Deserialize)]
pub struct UpdateSpecInput {
    /// Spec whose lifecycle state should change.
    pub spec_id: Option<String>,
    /// New lifecycle state.
    pub status: Option<String>,
    /// Optional explanation for the transition.
    pub note: Option<String>,
}

/// The set of valid status values a spec can transition to.
const VALID_STATUSES: &[&str] = &["active", "completed", "failed", "blocked"];

/// Transition `spec_id` to a new status, recording an optional note and
/// setting `completed_at` automatically when the status is terminal.
pub fn update_spec(db: &Database, input: UpdateSpecInput) -> ToolResult {
    let spec_id = input
        .spec_id
        .ok_or_else(|| ToolError::MissingField("spec_id".into()))?;
    let status = input
        .status
        .ok_or_else(|| ToolError::MissingField("status".into()))?;

    if !VALID_STATUSES.contains(&status.as_str()) {
        return Err(ToolError::InvalidValue(format!(
            "status must be one of: {}",
            VALID_STATUSES.join(", ")
        )));
    }

    let now = Utc::now().timestamp();
    let completed_at = if status == "completed" || status == "failed" {
        Some(now)
    } else {
        None
    };

    let rows = db
        .conn()
        .execute(
            "UPDATE specs SET status = ?1, status_note = ?2, completed_at = ?3 WHERE id = ?4",
            rusqlite::params![status, input.note, completed_at, spec_id],
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    if rows == 0 {
        return Err(ToolError::InvalidValue(format!(
            "Spec not found: {}",
            spec_id
        )));
    }

    Ok(Output::ok(format!("Spec {} marked as {}", spec_id, status)))
}

/// Input for `list_specs`: optional status filter and result cap (default 20).
#[derive(Deserialize)]
pub struct ListSpecsInput {
    /// Optional lifecycle state filter.
    pub status: Option<String>,
    /// Maximum number of rows to return.
    pub limit: Option<usize>,
}

/// Return specs ordered by creation time descending, optionally filtered to a
/// single status value. Each row includes its description, type, status, and timestamps.
pub fn list_specs(db: &Database, input: ListSpecsInput) -> ToolResult {
    let limit = input.limit.unwrap_or(20);

    let (query, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(ref status) =
        input.status
    {
        (
            "SELECT id, task_description, task_type, status, created_at, completed_at, status_note FROM specs WHERE status = ?1 ORDER BY created_at DESC LIMIT ?2",
            vec![Box::new(status.clone()), Box::new(limit as i64)],
        )
    } else {
        (
            "SELECT id, task_description, task_type, status, created_at, completed_at, status_note FROM specs ORDER BY created_at DESC LIMIT ?1",
            vec![Box::new(limit as i64)],
        )
    };

    let mut stmt = db
        .conn()
        .prepare(query)
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let rows = stmt
        .query_map(rusqlite::params_from_iter(params.iter()), |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "task_description": row.get::<_, String>(1)?,
                "task_type": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "created_at": row.get::<_, i64>(4)?,
                "completed_at": row.get::<_, Option<i64>>(5)?,
                "status_note": row.get::<_, Option<String>>(6)?,
            }))
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let results: Vec<_> = rows.filter_map(|r| r.ok()).collect();

    let mut output = Output::ok(format!("Found {} specs", results.len()));
    output.data = Some(serde_json::json!({ "specs": results }));
    Ok(output)
}

/// Input for `get_spec`: the ID of the spec to retrieve.
#[derive(Deserialize)]
pub struct GetSpecInput {
    /// Spec identifier to load.
    pub spec_id: Option<String>,
}

/// Fetch a full spec by ID, joining in all related hypotheses, approaches,
/// session learnings, and verification records so the agent sees the complete
/// history for that task in one call.
pub fn get_spec(db: &Database, input: GetSpecInput) -> ToolResult {
    let spec_id = input
        .spec_id
        .ok_or_else(|| ToolError::MissingField("spec_id".into()))?;

    let spec: serde_json::Value = db
        .conn()
        .query_row(
            "SELECT id, task_description, task_type, acceptance_criteria,
                    interface_contract, edge_cases, files_to_touch, dependencies,
                    unchanged_behaviors, implementation_tasks, test_properties,
                    status, created_at, completed_at, status_note
             FROM specs WHERE id = ?1",
            rusqlite::params![spec_id],
            |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "task_description": row.get::<_, String>(1)?,
                    "task_type": row.get::<_, String>(2)?,
                    "acceptance_criteria": row.get::<_, String>(3)?,
                    "interface_contract": row.get::<_, Option<String>>(4)?,
                    "edge_cases": row.get::<_, Option<String>>(5)?,
                    "files_to_touch": row.get::<_, Option<String>>(6)?,
                    "dependencies": row.get::<_, Option<String>>(7)?,
                    "unchanged_behaviors": row.get::<_, String>(8)?,
                    "implementation_tasks": row.get::<_, String>(9)?,
                    "test_properties": row.get::<_, String>(10)?,
                    "status": row.get::<_, String>(11)?,
                    "created_at": row.get::<_, i64>(12)?,
                    "completed_at": row.get::<_, Option<i64>>(13)?,
                    "status_note": row.get::<_, Option<String>>(14)?,
                }))
            },
        )
        .map_err(|e| ToolError::DatabaseError(format!("Spec not found: {}", e)))?;

    // Get related hypotheses
    let mut hyp_stmt = db
        .conn()
        .prepare(
            "SELECT id, hypothesis, outcome, confidence FROM hypotheses WHERE spec_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let hypotheses: Vec<serde_json::Value> = hyp_stmt
        .query_map(rusqlite::params![spec_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "hypothesis": row.get::<_, String>(1)?,
                "outcome": row.get::<_, Option<String>>(2)?,
                "confidence": row.get::<_, f64>(3)?,
            }))
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

    // Get related approaches
    let mut app_stmt = db
        .conn()
        .prepare(
            "SELECT id, name, score, chosen FROM approaches WHERE spec_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let approaches: Vec<serde_json::Value> = app_stmt
        .query_map(rusqlite::params![spec_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "score": row.get::<_, Option<f64>>(2)?,
                "chosen": row.get::<_, i64>(3)?,
            }))
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

    // Get related learnings
    let mut learn_stmt = db
        .conn()
        .prepare(
            "SELECT id, discovery FROM session_learns WHERE spec_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let learnings: Vec<serde_json::Value> = learn_stmt
        .query_map(rusqlite::params![spec_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "discovery": row.get::<_, String>(1)?,
            }))
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

    // Get related verifications
    let mut ver_stmt = db
        .conn()
        .prepare(
            "SELECT id, command, success, duration_ms, criteria_index FROM verifications WHERE spec_id = ?1 ORDER BY created_at DESC",
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    let verifications: Vec<serde_json::Value> = ver_stmt
        .query_map(rusqlite::params![spec_id], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "command": row.get::<_, String>(1)?,
                "success": row.get::<_, bool>(2)?,
                "duration_ms": row.get::<_, Option<i64>>(3)?,
                "criteria_index": row.get::<_, Option<i64>>(4)?,
            }))
        })
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?
        .filter_map(|r| r.ok())
        .collect();

    let mut output = Output::ok(format!("Spec {}", spec_id));
    output.data = Some(serde_json::json!({
        "spec": spec,
        "hypotheses": hypotheses,
        "approaches": approaches,
        "learnings": learnings,
        "verifications": verifications,
    }));
    Ok(output)
}

#[cfg(test)]
/// Validation and persistence tests for structured spec-artifact fields.
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// Build the smallest valid input while allowing one test to replace the
    /// artifact-only collections.
    fn input() -> SpecTaskInput {
        SpecTaskInput {
            task_description: Some("Add traceable artifacts".into()),
            task_type: Some("feature".into()),
            acceptance_criteria: Some(vec!["renders".into(), "links evidence".into()]),
            interface_contract: Some("spec_artifacts(spec_id)".into()),
            edge_cases: Some(vec![
                "old DB".into(),
                "no tasks".into(),
                "failed check".into(),
            ]),
            files_to_touch: Some(vec!["src/emit.rs".into()]),
            dependencies: Some("serde".into()),
            unchanged_behaviors: Some(vec!["old callers remain valid".into()]),
            implementation_tasks: Some(vec![ImplementationTask {
                description: "Render documents".into(),
                criteria_indices: vec![0, 1],
            }]),
            test_properties: Some(vec![TestProperty {
                description: "Every linked criterion appears".into(),
                criteria_index: 1,
            }]),
        }
    }

    /// Structured fields round-trip through the database and `get_spec`.
    #[test]
    fn artifact_fields_round_trip() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let created = spec_task(&db, input()).unwrap();
        let spec_id = created.id.unwrap();
        let loaded = get_spec(
            &db,
            GetSpecInput {
                spec_id: Some(spec_id),
            },
        )
        .unwrap();
        let spec = &loaded.data.unwrap()["spec"];

        assert_eq!(
            serde_json::from_str::<Vec<String>>(spec["unchanged_behaviors"].as_str().unwrap())
                .unwrap(),
            vec!["old callers remain valid"]
        );
        assert_eq!(
            serde_json::from_str::<Vec<ImplementationTask>>(
                spec["implementation_tasks"].as_str().unwrap()
            )
            .unwrap()[0]
                .criteria_indices,
            vec![0, 1]
        );
        assert_eq!(
            serde_json::from_str::<Vec<TestProperty>>(spec["test_properties"].as_str().unwrap())
                .unwrap()[0]
                .criteria_index,
            1
        );
    }

    /// Existing callers that omit artifact fields persist empty collections.
    #[test]
    fn omitted_artifact_fields_default_to_empty_collections() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let mut old_input = input();
        old_input.unchanged_behaviors = None;
        old_input.implementation_tasks = None;
        old_input.test_properties = None;
        let created = spec_task(&db, old_input).unwrap();

        let stored: (String, String, String) = db
            .conn()
            .query_row(
                "SELECT unchanged_behaviors, implementation_tasks, test_properties
                 FROM specs WHERE id = ?1",
                [created.id.unwrap()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored, ("[]".into(), "[]".into(), "[]".into()));
    }

    /// Tasks must link at least one real acceptance criterion.
    #[test]
    fn rejects_invalid_task_criterion_links() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let mut invalid = input();
        invalid.implementation_tasks = Some(vec![ImplementationTask {
            description: "Impossible task".into(),
            criteria_indices: vec![2],
        }]);

        let error = spec_task(&db, invalid)
            .err()
            .expect("an invalid task link must be rejected");
        assert!(error.to_string().contains("missing criterion 2"));
    }

    /// Property-test candidates cannot point outside the acceptance criteria.
    #[test]
    fn rejects_invalid_property_criterion_links() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let mut invalid = input();
        invalid.test_properties = Some(vec![TestProperty {
            description: "Impossible property".into(),
            criteria_index: 9,
        }]);

        let error = spec_task(&db, invalid)
            .err()
            .expect("an invalid property link must be rejected");
        assert!(error.to_string().contains("missing criterion 9"));
    }
}
