//! `verify`, `challenge_code`, and `session_diff` -- the post-edit gate tools.
//! `verify` runs commands and records pass/fail per spec criterion; `challenge_code`
//! emits an adversarial review prompt with a mechanical comment-coverage report;
//! `session_diff` summarises git changes before declaring a task done.

use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use chrono::Utc;
use serde::Deserialize;
use std::process::Command;
use std::time::Instant;
use uuid::Uuid;

/// Input for `verify`: a single command or a list of `steps`, plus optional
/// linkage to a spec criterion or skill execution record.
#[derive(Deserialize)]
pub struct VerifyInput {
    pub command: Option<String>,
    pub expected_exit_code: Option<i32>,
    pub skill_id: Option<i64>,
    pub spec_id: Option<String>,
    pub criteria_index: Option<i64>,
    pub timeout_secs: Option<u64>,
    pub steps: Option<Vec<VerifyStep>>,
}

/// One verification step: command line, the exit code that means success, and an optional label.
#[derive(Deserialize, Clone)]
pub struct VerifyStep {
    pub command: String,
    pub expected_exit_code: Option<i32>,
    pub label: Option<String>,
}

/// Outcome of a single executed step: timing, captured stdout/stderr, and success flag.
struct StepResult {
    command: String,
    label: Option<String>,
    success: bool,
    exit_code: i32,
    expected_exit_code: i32,
    duration_ms: i64,
    stdout: String,
    stderr: String,
}

/// Execute one `VerifyStep` directly (no shell), honouring an optional timeout.
fn run_step(step: &VerifyStep, timeout_secs: Option<u64>) -> Result<StepResult, ToolError> {
    // SECURITY (SEC-C1): parse command into argv and execute directly without
    // a shell. No shell injection from LLM-generated input.
    //
    // FORGE-2 fix: use shlex::split instead of split_whitespace so that
    // quoted arguments with embedded spaces are kept intact. For example,
    // `grep "foo bar" file` must produce ["grep", "foo bar", "file"], not
    // ["grep", "\"foo", "bar\"", "file"].
    let parts: Vec<String> = shlex::split(&step.command)
        .ok_or_else(|| ToolError::InvalidValue(format!("malformed command: {}", step.command)))?;
    if parts.is_empty() {
        return Err(ToolError::InvalidValue("empty command".into()));
    }

    let start = Instant::now();

    if let Some(secs) = timeout_secs {
        // Timeout path: spawn child, wait on separate thread, kill by PID on timeout
        let child = Command::new(&parts[0])
            .args(&parts[1..])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::IoError(e.to_string()))?;

        let timeout = std::time::Duration::from_secs(secs);
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let result = child.wait_with_output();
            let _ = tx.send(result);
        });

        match rx.recv_timeout(timeout) {
            Ok(Ok(output)) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                let actual = output.status.code().unwrap_or(-1);
                let expected = step.expected_exit_code.unwrap_or(0);
                Ok(StepResult {
                    command: step.command.clone(),
                    label: step.label.clone(),
                    success: actual == expected,
                    exit_code: actual,
                    expected_exit_code: expected,
                    duration_ms,
                    stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                    stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                })
            }
            Ok(Err(e)) => Err(ToolError::IoError(e.to_string())),
            Err(_) => {
                // Timeout -- child was moved into thread; process may orphan but
                // the thread will eventually finish. Acceptable for a CLI tool.
                Err(ToolError::IoError(format!(
                    "Command timed out after {}s: {}",
                    secs, step.command
                )))
            }
        }
    } else {
        // No timeout: simple blocking execution
        let output = Command::new(&parts[0])
            .args(&parts[1..])
            .output()
            .map_err(|e| ToolError::IoError(e.to_string()))?;

        let duration_ms = start.elapsed().as_millis() as i64;
        let actual = output.status.code().unwrap_or(-1);
        let expected = step.expected_exit_code.unwrap_or(0);
        Ok(StepResult {
            command: step.command.clone(),
            label: step.label.clone(),
            success: actual == expected,
            exit_code: actual,
            expected_exit_code: expected,
            duration_ms,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

/// Run all verification steps, persist per-step records to the `verifications` table
/// when a `spec_id` is supplied, and return a structured pass/fail summary.
pub fn verify(db: &Database, input: VerifyInput) -> ToolResult {
    let skill_id = input.skill_id;
    let spec_id = input.spec_id;
    let criteria_index = input.criteria_index;
    let timeout_secs = input.timeout_secs;

    // Build step list
    let mut steps: Vec<VerifyStep> = Vec::new();

    if let Some(cmd) = input.command {
        steps.push(VerifyStep {
            command: cmd,
            expected_exit_code: input.expected_exit_code,
            label: None,
        });
    }

    if let Some(extra) = input.steps {
        steps.extend(extra);
    }

    if steps.is_empty() {
        return Err(ToolError::MissingField("command or steps required".into()));
    }

    // Run all steps
    let mut results: Vec<StepResult> = Vec::new();
    let mut all_passed = true;

    for step in &steps {
        let step_result = run_step(step, timeout_secs)?;
        if !step_result.success {
            all_passed = false;
        }
        results.push(step_result);
    }

    let total_duration_ms: i64 = results.iter().map(|r| r.duration_ms).sum();

    // Record to verifications table if spec_id provided
    if let Some(ref sid) = spec_id {
        for r in &results {
            let id = format!("ver_{}", &Uuid::new_v4().to_string()[..8]);
            let now = Utc::now().timestamp();
            // Clip on a char boundary: subprocess output is arbitrary bytes and
            // a raw &s[..4096] panics when 4096 lands mid-multibyte-char.
            let stdout_clip = clip_bytes(&r.stdout, 4096);
            let stderr_clip = clip_bytes(&r.stderr, 4096);
            let _ = db.conn().execute(
                "INSERT INTO verifications (id, spec_id, created_at, command, exit_code, success, duration_ms, criteria_index, stdout, stderr) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    id, sid, now, r.command, r.exit_code,
                    r.success as i32, r.duration_ms, criteria_index,
                    stdout_clip,
                    stderr_clip,
                ],
            );
        }
    }

    // Skill recording
    if let Some(sid) = skill_id {
        if let Ok(client) = crate::kleos_client::KleosClient::new() {
            let err_msg = if all_passed {
                None
            } else {
                results
                    .iter()
                    .find(|r| !r.success)
                    .map(|r| r.stderr.as_str())
            };
            let _ = client.record_execution(
                sid,
                all_passed,
                Some(total_duration_ms as f64),
                if all_passed {
                    None
                } else {
                    Some("verify_failed")
                },
                err_msg.filter(|s| !s.trim().is_empty()),
            );
        }
    }

    // Build output
    let step_data: Vec<serde_json::Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "command": r.command,
                "label": r.label,
                "success": r.success,
                "exit_code": r.exit_code,
                "expected_exit_code": r.expected_exit_code,
                "duration_ms": r.duration_ms,
                "stdout": r.stdout.trim(),
                "stderr": r.stderr.trim(),
            })
        })
        .collect();

    let passed = results.iter().filter(|r| r.success).count();
    let total = results.len();

    let mut output = if all_passed {
        Output::ok(format!("Verification passed ({}/{} steps)", passed, total))
    } else {
        Output::error(format!(
            "Verification failed ({}/{} steps passed)",
            passed, total
        ))
    };

    output.data = Some(serde_json::json!({
        "all_passed": all_passed,
        "passed": passed,
        "total": total,
        "total_duration_ms": total_duration_ms,
        "steps": step_data,
    }));

    Ok(output)
}

/// Input for `challenge_code`: target file plus optional override of the focus list.
#[derive(Deserialize)]
pub struct ChallengeCodeInput {
    pub file_path: Option<String>,
    pub focus_areas: Option<Vec<String>>,
}

/// Build an adversarial review prompt for `file_path`, defaulting focus to
/// security/perf/error-handling/edge-cases/comment-coverage and embedding a
/// mechanical comment-coverage report so the reviewer sees concrete gaps.
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
            "comment_coverage".into(),
        ]
    });

    // Read the file for line count and comment scan
    let content = std::fs::read_to_string(&file_path)
        .map_err(|e| ToolError::IoError(format!("Cannot read {}: {}", file_path, e)))?;

    let lines = content.lines().count();

    // Mechanically scan for undocumented declarations so the reviewer sees concrete gaps.
    let comment_report = crate::tools::comments::comment_check(
        _db,
        crate::tools::comments::CommentCheckInput {
            file_path: Some(file_path.clone()),
        },
    )
    .ok();

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
        "comment_report": comment_report.and_then(|o| o.data),
        "prompt": format!(
            "Adversarially review this code for issues in: {}. \
            Find real problems, not style nits. \
            For each issue: describe it, explain impact, suggest fix. \
            \
            HARD RULE -- comment coverage: every declaration (fn, struct, enum, \
            trait, impl, mod, type, class, method) MUST be preceded by a comment \
            describing what the code does. Every non-trivial source file MUST \
            have a module/file header comment stating its role. Treat any missing \
            comment as a real problem, not a style nit. List each undocumented \
            declaration by line number and propose the comment to add.",
            focus.join(", ")
        ),
    }));

    Ok(output)
}

/// Input for `session_diff`: optional base ref to diff against (default `HEAD~10`).
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

/// Summarise changes between `HEAD` and `base` (default `HEAD~10`): a `--stat`
/// digest plus the list of changed files. Used as the final pre-done audit step.
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

/// Clip a string to at most `max` bytes without splitting a UTF-8 character.
/// Subprocess output is arbitrary, so a raw byte slice can panic on a
/// multibyte boundary; this walks back to the nearest char boundary.
fn clip_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
