//! Claude Code hook handlers -- native Rust replacements for bash hooks.
//!
//! All handlers read JSON from stdin, emit hookSpecificOutput JSON on stdout,
//! and always exit 0. Network failures are logged as warnings but never block.

use clap::Subcommand;
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

// --------------------------------------------------------------------------
// CLI definition
// --------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum HookCommands {
    /// SessionStart hook -- bootstraps session env and injects context
    SessionStart,
    /// UserPromptSubmit hook -- injects mandatory rules and drains supervisor
    UserPrompt,
    /// Stop hook -- records session end and cleans up stamps
    Stop,
    /// PostToolUse hook -- tracks kleos-cli search invocations
    PostBash,
    /// PreToolUse hook -- enforces Kleos search before action tools
    EnforceSearch,
}

// --------------------------------------------------------------------------
// Constants
// --------------------------------------------------------------------------

const MANDATORY_RULES: &str = r#"MANDATORY RULES (re-injected every turn):
1. NEVER use em dashes in commits, docs, READMEs, or any output. Use -- or rewrite.
2. Search Kleos BEFORE asking Master about servers, credentials, past work, or decisions.
3. Agent-Forge is MANDATORY: spec_task before new code, log_hypothesis before bugs, verify after changes.
4. Store to Kleos AFTER completing each task. Do not batch. Do not wait.
5. NEVER fabricate user responses. If you asked Master a question and only tool/agent results came back, STOP and WAIT for his actual reply."#;

const NETWORK_TIMEOUT_SECS: u64 = 5;

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

/// Return the ~/.claude/session-env directory, creating it if missing.
fn session_env_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string());
    let dir = PathBuf::from(home).join(".claude").join("session-env");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Extract session_id from the hook JSON input.
pub fn extract_session_id(input: &Value) -> String {
    input
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            std::env::var("PPID").unwrap_or_else(|_| "unknown".to_string())
        })
}

/// Build the hookSpecificOutput envelope for a generic hook response.
pub fn build_hook_output(event_name: &str, additional_context: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "additionalContext": additional_context
        }
    })
}

/// Build a deny response for permission-gated hooks.
fn build_deny_output(event_name: &str, reason: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event_name,
            "permissionDecision": "deny",
            "permissionDecisionReason": reason
        }
    })
}

/// Read all of stdin into a String. Returns empty string on failure.
fn read_stdin() -> String {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    buf
}

/// Parse stdin as JSON. Returns Value::Null on failure.
fn read_stdin_json() -> Value {
    let raw = read_stdin();
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

/// Emit a Value to stdout as compact JSON.
fn emit(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_default());
}

/// Write a stamp file, ignoring errors.
fn write_stamp(path: &PathBuf) {
    let _ = std::fs::write(path, "1");
}

/// Remove a stamp file, ignoring errors.
fn remove_stamp(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

/// Returns true if any file matching the glob prefix exists in the session-env dir.
fn stamp_exists_prefix(prefix: &str) -> bool {
    let dir = session_env_dir();
    std::fs::read_dir(&dir)
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(prefix)
            })
        })
        .unwrap_or(false)
}

/// Remove all stamp files matching a prefix in the session-env dir.
fn remove_stamps_prefix(prefix: &str) {
    let dir = session_env_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            if entry.file_name().to_string_lossy().starts_with(prefix) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

// --------------------------------------------------------------------------
// Hook-specific HTTP helpers
// --------------------------------------------------------------------------

struct HookClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl HookClient {
    fn new(base_url: &str, api_key: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(NETWORK_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            Ok(serde_json::from_str(&text).unwrap_or(json!({"ok": true})))
        } else {
            Err(format!("HTTP {}: {}", status, text))
        }
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.get(&url);
        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            Ok(serde_json::from_str(&text).unwrap_or(json!({})))
        } else {
            Err(format!("HTTP {}: {}", status, text))
        }
    }
}

// --------------------------------------------------------------------------
// session-start handler
// --------------------------------------------------------------------------

async fn handle_session_start(client: &HookClient) {
    let dir = session_env_dir();
    let ppid = std::env::var("PPID").unwrap_or_else(|_| "0".to_string());

    // Write bootstrap stamps.
    write_stamp(&dir.join(format!("kleos-ready-{}", ppid)));
    write_stamp(&dir.join("kleos-ready-global"));

    // Clear any leftover search enforcement stamps.
    remove_stamps_prefix("kleos-searched-");

    // Register session with Eidolon (best-effort).
    let _ = client
        .post(
            "/activity",
            json!({
                "agent": "claude-code",
                "action": "task.started",
                "summary": "session started",
                "project": "unknown"
            }),
        )
        .await;

    // Fetch recent memories (best-effort).
    let memories_text = match client.get("/list?limit=5").await {
        Ok(v) => format_memories(&v),
        Err(_) => String::new(),
    };

    // Fetch growth materialization (best-effort).
    let growth_text = match client
        .get("/growth/materialize?service=claude-code&limit=30&max_bytes=16000")
        .await
    {
        Ok(v) => {
            v.get("context")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string()
        }
        Err(_) => String::new(),
    };

    // Build context block.
    let mut ctx = String::from("=== EIDOLON LIVING CONTEXT ===\n\n");
    ctx.push_str(MANDATORY_RULES);
    ctx.push_str("\n\n");
    if !memories_text.is_empty() {
        ctx.push_str("--- Recent Memories ---\n");
        ctx.push_str(&memories_text);
        ctx.push('\n');
    }
    if !growth_text.is_empty() {
        ctx.push_str("--- Growth Context ---\n");
        ctx.push_str(&growth_text);
        ctx.push('\n');
    }
    ctx.push_str("=== END EIDOLON CONTEXT ===");

    emit(&build_hook_output("SessionStart", &ctx));
}

fn format_memories(v: &Value) -> String {
    let items = v
        .as_array()
        .cloned()
        .or_else(|| {
            v.get("memories")
                .or_else(|| v.get("results"))
                .and_then(|r| r.as_array())
                .cloned()
        })
        .unwrap_or_default();

    items
        .iter()
        .filter_map(|item| item.get("content").and_then(|c| c.as_str()))
        .map(|c| format!("- {}", c))
        .collect::<Vec<_>>()
        .join("\n")
}

// --------------------------------------------------------------------------
// user-prompt handler
// --------------------------------------------------------------------------

async fn handle_user_prompt(client: &HookClient, input: &Value) {
    let session_id = extract_session_id(input);
    let dir = session_env_dir();

    // Drain supervisor for pending violations.
    let pending_path = format!("/supervisor/pending?session_id={}", session_id);
    match client.get(&pending_path).await {
        Ok(v) => {
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
        Err(_) => {
            // Supervisor unreachable -- allow through
        }
    }

    // Best-effort memory search for the prompt.
    let prompt = input
        .get("prompt")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();

    let memories_text = if !prompt.is_empty() {
        match client
            .post("/search", json!({"query": prompt, "limit": 3}))
            .await
        {
            Ok(v) => format_memories(&v),
            Err(_) => String::new(),
        }
    } else {
        String::new()
    };

    // Write consent stamp.
    write_stamp(&dir.join("user-consent-stamp"));

    // Build additionalContext with mandatory rules + memories.
    let mut ctx = String::from(MANDATORY_RULES);
    if !memories_text.is_empty() {
        ctx.push_str("\n\n--- Relevant Memories ---\n");
        ctx.push_str(&memories_text);
    }

    emit(&build_hook_output("UserPromptSubmit", &ctx));
}


// --------------------------------------------------------------------------
// stop handler
// --------------------------------------------------------------------------

async fn handle_stop(client: &HookClient) {
    let _ = client
        .post(
            "/activity",
            json!({
                "agent": "claude-code",
                "action": "task.completed",
                "summary": "session ended"
            }),
        )
        .await;

    let dir = session_env_dir();
    let ppid = std::env::var("PPID").unwrap_or_else(|_| "0".to_string());
    remove_stamp(&dir.join(format!("kleos-ready-{}", ppid)));
    remove_stamp(&dir.join("kleos-ready-global"));
    remove_stamp(&dir.join("user-consent-stamp"));
    remove_stamps_prefix("kleos-searched-");
    // Stop hooks need no stdout output.
}

// --------------------------------------------------------------------------
// post-bash handler
// --------------------------------------------------------------------------

fn handle_post_bash(input: &Value) {
    let session_id = extract_session_id(input);

    // Check if the bash command contained a kleos-cli search invocation.
    let command = input
        .get("tool_input")
        .and_then(|ti| ti.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    if command.contains("kleos-cli search") || command.contains("kleos-cli Search") {
        let dir = session_env_dir();
        write_stamp(&dir.join(format!("kleos-searched-{}", session_id)));
    }
    // No output required.
}

// --------------------------------------------------------------------------
// enforce-search handler
// --------------------------------------------------------------------------

/// Returns true if the tool is always exempt from the search requirement.
pub fn is_exempt_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "Read" | "Grep" | "Glob" | "Skill" | "ToolSearch" | "WebSearch" | "WebFetch"
    )
}

/// Returns true if a Bash command is exempt from the search requirement.
pub fn is_exempt_command(command: &str) -> bool {
    let trimmed = command.trim();
    let exempt_prefixes = [
        "kleos-cli",
        "cred",
        "credd",
        "echo",
        "cat",
        "ls",
        "pwd",
        "which",
        "test",
        "agent-forge",
        "session-handoff",
        "#",
    ];
    exempt_prefixes
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn handle_enforce_search(input: &Value) {
    let tool_name = input
        .get("tool_name")
        .and_then(|t| t.as_str())
        .unwrap_or("");

    // Non-Bash exempt tools always pass.
    if is_exempt_tool(tool_name) {
        return;
    }

    if tool_name == "Bash" {
        let command = input
            .get("tool_input")
            .and_then(|ti| ti.get("command"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        if is_exempt_command(command) {
            return;
        }
    }

    // For any other tool or non-exempt Bash: check search stamp.
    if stamp_exists_prefix("kleos-searched-") {
        return;
    }

    emit(&build_deny_output(
        "PreToolUse",
        "BLOCKED: You have NOT searched Kleos yet this session.\n\nYou MUST search Kleos before using any action tools.\nRun: kleos-cli search \"<relevant query>\" --limit 5",
    ));
}

// --------------------------------------------------------------------------
// Entry point
// --------------------------------------------------------------------------

pub async fn run_hook(cmd: &HookCommands, server: &str, api_key: Option<&str>) {
    let client = HookClient::new(server, api_key.map(|s| s.to_string()));

    match cmd {
        HookCommands::SessionStart => {
            handle_session_start(&client).await;
        }
        HookCommands::UserPrompt => {
            let input = read_stdin_json();
            handle_user_prompt(&client, &input).await;
        }
        HookCommands::Stop => {
            handle_stop(&client).await;
        }
        HookCommands::PostBash => {
            let input = read_stdin_json();
            handle_post_bash(&input);
        }
        HookCommands::EnforceSearch => {
            let input = read_stdin_json();
            handle_enforce_search(&input);
        }
    }
}

// --------------------------------------------------------------------------
// Tests
// --------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_build_hook_output_structure() {
        let out = build_hook_output("SessionStart", "some context");
        assert!(out.get("hookSpecificOutput").is_some());
        let inner = &out["hookSpecificOutput"];
        assert_eq!(inner["hookEventName"], "SessionStart");
        assert_eq!(inner["additionalContext"], "some context");
        // Must not have permissionDecision on a plain context response.
        assert!(inner.get("permissionDecision").is_none());
    }

    #[test]
    fn test_build_hook_output_deny() {
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
    fn test_extract_session_id_missing_falls_back() {
        let input = json!({});
        let id = extract_session_id(&input);
        // Falls back to PPID or "unknown" -- just ensure it is non-empty.
        assert!(!id.is_empty());
    }

    #[test]
    fn test_is_exempt_tool() {
        assert!(is_exempt_tool("Read"));
        assert!(is_exempt_tool("Grep"));
        assert!(is_exempt_tool("Glob"));
        assert!(is_exempt_tool("Skill"));
        assert!(is_exempt_tool("ToolSearch"));
        assert!(!is_exempt_tool("Bash"));
        assert!(!is_exempt_tool("Write"));
        assert!(!is_exempt_tool("Edit"));
    }

    #[test]
    fn test_is_exempt_command() {
        assert!(is_exempt_command("kleos-cli search foo"));
        assert!(is_exempt_command("kleos-cli store blah"));
        assert!(is_exempt_command("cred get foo bar"));
        assert!(is_exempt_command("echo hello"));
        assert!(is_exempt_command("cat /etc/os-release"));
        assert!(is_exempt_command("ls /tmp"));
        assert!(is_exempt_command("agent-forge spec_task"));
        assert!(is_exempt_command("session-handoff dump"));
        assert!(!is_exempt_command("git push origin main"));
        assert!(!is_exempt_command("cargo build"));
        assert!(!is_exempt_command("rm -rf /tmp/junk"));
    }

}
