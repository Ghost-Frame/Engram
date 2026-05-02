use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use serde::Deserialize;
use std::process::Command;

#[derive(Deserialize)]
pub struct VerifyInput {
    pub command: Option<String>,
    pub expected_exit_code: Option<i32>,
}

pub fn verify(_db: &Database, input: VerifyInput) -> ToolResult {
    let command = input
        .command
        .ok_or_else(|| ToolError::MissingField("command".into()))?;

    let expected = input.expected_exit_code.unwrap_or(0);

    // SECURITY (SEC-C1): parse command into argv and execute directly without
    // a shell. The previous `sh -c` invocation allowed arbitrary shell injection
    // from LLM-generated input (prompt injection attack surface).
    let parts: Vec<&str> = command.split_whitespace().collect();
    if parts.is_empty() {
        return Err(ToolError::InvalidValue("empty command".into()));
    }
    let output = Command::new(parts[0])
        .args(&parts[1..])
        .output()
        .map_err(|e| ToolError::IoError(e.to_string()))?;

    let actual = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let success = actual == expected;

    let mut result = if success {
        Output::ok("Verification passed")
    } else {
        Output::error(format!(
            "Verification failed: expected exit code {}, got {}",
            expected, actual
        ))
    };

    result.data = Some(serde_json::json!({
        "exit_code": actual,
        "stdout": stdout.trim(),
        "stderr": stderr.trim(),
    }));

    Ok(result)
}

#[derive(Deserialize)]
pub struct ChallengeCodeInput {
    pub file_path: Option<String>,
    pub focus_areas: Option<Vec<String>>,
}

pub fn challenge_code(_db: &Database, input: ChallengeCodeInput) -> ToolResult {
    let file_path = input
        .file_path
        .ok_or_else(|| ToolError::MissingField("file_path".into()))?;

    let focus = input.focus_areas.unwrap_or_else(|| {
        vec![
            "security".into(),
            "performance".into(),
            "error_handling".into(),
            "edge_cases".into(),
        ]
    });

    // Read the file
    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| ToolError::IoError(format!("Cannot read {}: {}", file_path, e)))?;

    let lines = content.lines().count();

    let mut output = Output::ok(format!(
        "Challenge: Review {} ({} lines) for: {}",
        file_path,
        lines,
        focus.join(", ")
    ));
    output.data = Some(serde_json::json!({
        "file": file_path,
        "lines": lines,
        "focus_areas": focus,
        "prompt": format!(
            "Adversarially review this code for issues in: {}. \
            Find real problems, not style nits. \
            For each issue: describe it, explain impact, suggest fix.",
            focus.join(", ")
        ),
    }));

    Ok(output)
}

#[derive(Deserialize)]
pub struct SessionDiffInput {
    pub base: Option<String>,
}

/// Validate a git ref to prevent flag injection or shell metacharacters.
fn validate_git_ref(s: &str) -> std::result::Result<(), ToolError> {
    if s.len() > 100 {
        return Err(ToolError::InvalidValue("git ref too long".into()));
    }
    // Allow alphanumeric, dash, underscore, dot, slash, tilde, caret, colon
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.~/^:@{}".contains(c))
    {
        return Err(ToolError::InvalidValue(
            "git ref contains disallowed characters".into(),
        ));
    }
    // Reject refs that look like flags
    if s.starts_with('-') {
        return Err(ToolError::InvalidValue(
            "git ref must not start with '-'".into(),
        ));
    }
    Ok(())
}

pub fn session_diff(_db: &Database, input: SessionDiffInput) -> ToolResult {
    let base = input.base.unwrap_or_else(|| "HEAD~10".into());
    // SECURITY: validate the ref to prevent flag injection into git args.
    validate_git_ref(&base)?;

    let output = Command::new("git")
        .args(["diff", "--stat", &base])
        .output()
        .map_err(|e| ToolError::IoError(e.to_string()))?;

    let diff_stat = String::from_utf8_lossy(&output.stdout);

    let files_output = Command::new("git")
        .args(["diff", "--name-only", &base])
        .output()
        .map_err(|e| ToolError::IoError(e.to_string()))?;

    let files: Vec<String> = String::from_utf8_lossy(&files_output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect();

    let mut result = Output::ok(format!("{} files changed since {}", files.len(), base));
    result.data = Some(serde_json::json!({
        "base": base,
        "files": files,
        "stat": diff_stat.trim(),
    }));

    Ok(result)
}
