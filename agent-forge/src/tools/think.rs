use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ThinkInput {
    pub problem: Option<String>,
    pub constraints: Option<Vec<String>>,
    pub context: Option<String>,
}

pub fn think(_db: &Database, input: ThinkInput) -> ToolResult {
    let problem = input
        .problem
        .ok_or_else(|| ToolError::MissingField("problem".into()))?;

    let constraints = input.constraints.unwrap_or_default();

    let mut output = Output::ok("Structured reasoning prompt generated");
    output.data = Some(serde_json::json!({
        "problem": problem,
        "constraints": constraints,
        "context": input.context,
        "prompt": format!(
            "Think through this problem step by step:\n\n\
            PROBLEM: {}\n\n\
            CONSTRAINTS:\n{}\n\n\
            CONTEXT: {}\n\n\
            Reason through:\n\
            1. What do I know?\n\
            2. What do I need to find out?\n\
            3. What are the options?\n\
            4. What are the tradeoffs?\n\
            5. What is my recommendation and why?",
            problem,
            constraints.iter().map(|c| format!("- {}", c)).collect::<Vec<_>>().join("\n"),
            input.context.unwrap_or_else(|| "None provided".into()),
        ),
    }));

    Ok(output)
}

#[derive(Deserialize)]
pub struct DeclareUnknownsInput {
    pub unknowns: Option<Vec<UnknownItem>>,
}

#[derive(Deserialize)]
pub struct UnknownItem {
    pub description: String,
    pub blocking: bool,
    pub resolution_hint: Option<String>,
}

pub fn declare_unknowns(_db: &Database, input: DeclareUnknownsInput) -> ToolResult {
    let unknowns = input
        .unknowns
        .ok_or_else(|| ToolError::MissingField("unknowns".into()))?;

    if unknowns.is_empty() {
        return Err(ToolError::InvalidValue(
            "At least one unknown must be declared".into(),
        ));
    }

    let blocking: Vec<_> = unknowns.iter().filter(|u| u.blocking).collect();
    let non_blocking: Vec<_> = unknowns.iter().filter(|u| !u.blocking).collect();

    let mut output = Output::ok(format!(
        "Declared {} unknowns ({} blocking, {} non-blocking)",
        unknowns.len(),
        blocking.len(),
        non_blocking.len()
    ));

    output.data = Some(serde_json::json!({
        "blocking": blocking.iter().map(|u| {
            serde_json::json!({
                "description": u.description,
                "resolution_hint": u.resolution_hint,
            })
        }).collect::<Vec<_>>(),
        "non_blocking": non_blocking.iter().map(|u| {
            serde_json::json!({
                "description": u.description,
                "resolution_hint": u.resolution_hint,
            })
        }).collect::<Vec<_>>(),
        "action": if !blocking.is_empty() {
            "STOP: Resolve blocking unknowns before proceeding"
        } else {
            "OK: No blocking unknowns, proceed with caution on non-blocking items"
        },
    }));

    Ok(output)
}
