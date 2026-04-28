//! Claude Code hook handlers -- thin shim that routes decisions to kleos-server.
//! All handlers read JSON from stdin, call the server, emit hookSpecificOutput on stdout.
//! Network failures are logged (eprintln) but never block -- fail open, exit 0.

use clap::Subcommand;
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

use crate::Client;

// --------------------------------------------------------------------------
// CLI definition
// --------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum HookCommands {
    /// SessionStart hook -- registers session, fetches context
    SessionStart,
    /// UserPromptSubmit hook -- drains supervisor, injects mandatory rules
    UserPrompt,
    /// Stop hook -- records session end
    Stop,
    /// PreToolUse hook -- routes tool calls through /gate/check
    PreTool,
    /// PostToolUse hook -- reports activity, completes gate
    PostTool,
}

// --------------------------------------------------------------------------
// Constants
// --------------------------------------------------------------------------

// TODO: fetch from /policy/mandatory endpoint
const MANDATORY_RULES: &str = r#"MANDATORY RULES (re-injected every turn):
1. NEVER use em dashes in commits, docs, READMEs, or any output. Use -- or rewrite.
2. Search Kleos BEFORE asking Master about servers, credentials, past work, or decisions.
3. Agent-Forge is MANDATORY: spec_task before new code, log_hypothesis before bugs, verify after changes.
4. Store to Kleos AFTER completing each task. Do not batch. Do not wait.
5. NEVER fabricate user responses. If you asked Master a question and only tool/agent results came back, STOP and WAIT for his actual reply."#;

const GATE_TIMEOUT: Duration = Duration::from_secs(130);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

fn read_stdin_json() -> Value {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    serde_json::from_str(&buf).unwrap_or(Value::Null)
}

fn extract_session_id(input: &Value) -> String {
    input
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            std::env::var("PPID").unwrap_or_else(|_| "unknown".to_string())
        })
}

fn emit(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_default());
}

fn build_context_output(event: &str, context: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": context
        }
    })
}

fn build_deny_output(event: &str, reason: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "permissionDecision": "deny",
            "permissionDecisionReason": reason
        }
    })
}

/// Derive the "command" string from Claude Code's tool_input JSON.
/// For Bash: the literal command. For Write/Edit: "Write to <path>" or "Edit <path>".
/// For others: serialized summary.
fn derive_command(tool_name: &str, tool_input: &Value) -> String {
    match tool_name {
        "Bash" => tool_input
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        "Write" | "Edit" => {
            let path = tool_input
                .get("file_path")
                .and_then(|p| p.as_str())
                .unwrap_or("<unknown>");
            format!("{} {}", tool_name, path)
        }
        "WebFetch" => tool_input
            .get("url")
            .and_then(|u| u.as_str())
            .unwrap_or("WebFetch")
            .to_string(),
        "WebSearch" => tool_input
            .get("query")
            .and_then(|q| q.as_str())
            .unwrap_or("WebSearch")
            .to_string(),
        _ => format!("{}: {}", tool_name, serde_json::to_string(tool_input).unwrap_or_default()),
    }
}

// --------------------------------------------------------------------------
// Hook handlers
// --------------------------------------------------------------------------

async fn handle_session_start(client: &Client) {
    // Register session with activity (best-effort)
    let _ = client
        .post_with_timeout(
            "/activity",
            json!({
                "agent": "claude-code",
                "action": "session.start",
                "summary": "session started",
                "project": "unknown"
            }),
            DEFAULT_TIMEOUT,
        )
        .await;

    // Fetch growth context (best-effort)
    let growth_text = match client
        .get_with_timeout(
            "/growth/materialize?service=claude-code&limit=30&max_bytes=16000",
            DEFAULT_TIMEOUT,
        )
        .await
    {
        Ok(v) => v
            .get("context")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => String::new(),
    };

    let mut ctx = String::from("=== EIDOLON LIVING CONTEXT ===\n\n");
    ctx.push_str(MANDATORY_RULES);
    if !growth_text.is_empty() {
        ctx.push_str("\n\n--- Growth Context ---\n");
        ctx.push_str(&growth_text);
    }
    ctx.push_str("\n\n=== END EIDOLON CONTEXT ===");

    emit(&build_context_output("SessionStart", &ctx));
}

async fn handle_user_prompt(client: &Client, input: &Value) {
    let session_id = extract_session_id(input);

    // Drain supervisor for pending violations
    let pending_path = format!("/supervisor/pending?session_id={}", session_id);
    if let Ok(v) = client.get_with_timeout(&pending_path, DEFAULT_TIMEOUT).await {
        let injections = v
            .get("injections")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        if !injections.is_empty() {
            let msg = injections
                .first()
                .and_then(|vio| vio.get("message").and_then(|m| m.as_str()))
                .unwrap_or("policy violation detected");
            emit(&build_deny_output(
                "UserPromptSubmit",
                &format!("Supervisor violation: {}", msg),
            ));
            return;
        }
    }

    // Build context with mandatory rules
    emit(&build_context_output("UserPromptSubmit", MANDATORY_RULES));
}

async fn handle_stop(client: &Client) {
    let _ = client
        .post_with_timeout(
            "/activity",
            json!({
                "agent": "claude-code",
                "action": "session.end",
                "summary": "session ended"
            }),
            DEFAULT_TIMEOUT,
        )
        .await;
    // Stop hooks need no stdout output
}

async fn handle_pre_tool(client: &Client, input: &Value) {
    let tool_name = input
        .get("tool_name")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    let tool_input = input.get("tool_input").cloned().unwrap_or(json!({}));
    let session_id = extract_session_id(input);

    let command = derive_command(tool_name, &tool_input);

    // Derive agent name from signer (matches PIV enrollment)
    let agent = client.agent_label();

    let gate_body = json!({
        "command": command,
        "agent": agent,
        "tool_name": tool_name,
        "session_id": session_id,
        "context": format!("tool_input: {}", serde_json::to_string(&tool_input).unwrap_or_default()),
        "skip_approval": true,
    });

    let result = match client.post_with_timeout("/gate/check", gate_body, GATE_TIMEOUT).await {
        Ok(v) => v,
        Err(e) => {
            eprintln!("kleos hook pre-tool: gate unreachable ({}), allowing", e);
            return; // Fail open
        }
    };

    let allowed = result.get("allowed").and_then(|a| a.as_bool()).unwrap_or(true);
    let reason = result
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or("blocked by gate");
    let enrichment = result.get("enrichment").and_then(|e| e.as_str());

    if !allowed {
        emit(&build_deny_output("PreToolUse", reason));
    } else if let Some(enrich) = enrichment {
        emit(&build_context_output("PreToolUse", enrich));
    }
    // else: no output = implicit allow
}

async fn handle_post_tool(client: &Client, input: &Value) {
    let tool_name = input
        .get("tool_name")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown");
    let session_id = extract_session_id(input);

    // Report activity (best-effort)
    let _ = client
        .post_with_timeout(
            "/activity",
            json!({
                "agent": "claude-code",
                "action": "tool.completed",
                "summary": format!("{} completed", tool_name),
            }),
            DEFAULT_TIMEOUT,
        )
        .await;

    // Close latest open gate for this session (best-effort, idempotent)
    let _ = client
        .post_with_timeout(
            "/gate/complete-latest",
            json!({
                "session_id": session_id,
                "output": format!("{} completed", tool_name),
                "known_secrets": [],
            }),
            DEFAULT_TIMEOUT,
        )
        .await;
    // No stdout output for PostToolUse
}

// --------------------------------------------------------------------------
// Entry point
// --------------------------------------------------------------------------

pub async fn run_hook(cmd: &HookCommands, client: &Client) {
    match cmd {
        HookCommands::SessionStart => {
            handle_session_start(client).await;
        }
        HookCommands::UserPrompt => {
            let input = read_stdin_json();
            handle_user_prompt(client, &input).await;
        }
        HookCommands::Stop => {
            handle_stop(client).await;
        }
        HookCommands::PreTool => {
            let input = read_stdin_json();
            handle_pre_tool(client, &input).await;
        }
        HookCommands::PostTool => {
            let input = read_stdin_json();
            handle_post_tool(client, &input).await;
        }
    }
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_context_output_structure() {
        let out = build_context_output("SessionStart", "some context");
        let inner = &out["hookSpecificOutput"];
        assert_eq!(inner["hookEventName"], "SessionStart");
        assert_eq!(inner["additionalContext"], "some context");
        assert!(inner.get("permissionDecision").is_none());
    }

    #[test]
    fn test_build_deny_output_structure() {
        let out = build_deny_output("PreToolUse", "blocked!");
        let inner = &out["hookSpecificOutput"];
        assert_eq!(inner["hookEventName"], "PreToolUse");
        assert_eq!(inner["permissionDecision"], "deny");
        assert_eq!(inner["permissionDecisionReason"], "blocked!");
    }

    #[test]
    fn test_extract_session_id_present() {
        let input = json!({ "session_id": "abc-123" });
        assert_eq!(extract_session_id(&input), "abc-123");
    }

    #[test]
    fn test_extract_session_id_fallback() {
        let input = json!({});
        let id = extract_session_id(&input);
        assert!(!id.is_empty());
    }

    #[test]
    fn test_derive_command_bash() {
        let input = json!({"command": "ls -la"});
        assert_eq!(derive_command("Bash", &input), "ls -la");
    }

    #[test]
    fn test_derive_command_write() {
        let input = json!({"file_path": "/tmp/foo.rs"});
        assert_eq!(derive_command("Write", &input), "Write /tmp/foo.rs");
    }

    #[test]
    fn test_derive_command_edit() {
        let input = json!({"file_path": "/tmp/bar.rs"});
        assert_eq!(derive_command("Edit", &input), "Edit /tmp/bar.rs");
    }

    #[test]
    fn test_derive_command_other() {
        let input = json!({"url": "https://example.com"});
        let cmd = derive_command("WebFetch", &input);
        assert_eq!(cmd, "https://example.com");
    }
}
