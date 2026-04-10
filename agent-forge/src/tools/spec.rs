use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct SpecTaskInput {
    pub task_description: Option<String>,
    pub task_type: Option<String>,
    pub acceptance_criteria: Option<Vec<String>>,
    pub interface_contract: Option<String>,
    pub edge_cases: Option<Vec<String>>,
    pub files_to_touch: Option<Vec<String>>,
    pub dependencies: Option<String>,
}

const VALID_TASK_TYPES: &[&str] = &[
    "feature",
    "bugfix",
    "refactor",
    "enhancement",
    "test",
    "docs",
];

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

    let id = format!("spec_{}", &Uuid::new_v4().to_string()[..8]);
    let now = Utc::now().timestamp();

    db.conn()
        .execute(
            r#"
            INSERT INTO specs (id, created_at, task_description, task_type, acceptance_criteria, interface_contract, edge_cases, files_to_touch, dependencies, status)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'active')
            "#,
            rusqlite::params![
                id,
                now,
                task_description,
                task_type,
                serde_json::to_string(&acceptance_criteria).unwrap(),
                interface_contract,
                serde_json::to_string(&edge_cases).unwrap(),
                input.files_to_touch.map(|v| serde_json::to_string(&v).unwrap()),
                input.dependencies,
            ],
        )
        .map_err(|e| ToolError::DatabaseError(e.to_string()))?;

    Ok(Output::ok_with_id(id, "Spec created"))
}
