//! Claude Code hook handlers -- thin shim that routes decisions to kleos-server.
//! All handlers read JSON from stdin, call the server, emit hookSpecificOutput on stdout.
//! Network failures are logged (eprintln) but never block -- fail open, exit 0.

use clap::Subcommand;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

use crate::Client;

// --- CLI definition ---

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
    /// Back-compat alias for older packaged settings.
    #[command(name = "post-bash", hide = true)]
    PostBash,
}

// --- Constants ---

/// Offline / fetch-failure fallback for the mandatory rules text.
///
/// Empty by design: the rules are operator-configured server-side via the
/// `KLEOS_MANDATORY_RULES` env var. If the CLI cannot reach the server, no
/// rules are injected rather than substituting hardcoded content that may
/// not match the operator's policy.
const FALLBACK_MANDATORY_RULES: &str = "";

const POLICY_CACHE_TTL_SECS: u64 = 60;

const GATE_TIMEOUT: Duration = Duration::from_secs(130);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const SIDECAR_RECALL_TIMEOUT: Duration = Duration::from_secs(12);
const SIDECAR_OBSERVE_TIMEOUT: Duration = Duration::from_secs(5);
const SIDECAR_END_TIMEOUT: Duration = Duration::from_secs(15);

// --- Policy fetch with cache ---

/// Returns the mandatory rules text.
/// Tries `{server_url}/policy/mandatory` first (2s timeout).
/// On success, caches the response to `~/.cache/kleos/policy.json` (60s TTL).
/// On any failure, falls back to `FALLBACK_MANDATORY_RULES`.
async fn fetch_mandatory_rules(client: &Client) -> String {
    // Check cache first
    if let Some(cached) = read_policy_cache() {
        return cached;
    }

    let timeout = std::time::Duration::from_secs(2);
    match client.get_with_timeout("/policy/mandatory", timeout).await {
        Ok(v) => {
            let rules = v
                .get("rules")
                .and_then(|r| r.as_str())
                .unwrap_or(FALLBACK_MANDATORY_RULES)
                .to_string();
            write_policy_cache(&rules);
            rules
        }
        Err(e) => {
            eprintln!(
                "kleos hook: /policy/mandatory fetch failed ({}), using fallback",
                e
            );
            FALLBACK_MANDATORY_RULES.to_string()
        }
    }
}

/// Returns the on-disk cache path for mandatory hook policy text.
fn policy_cache_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = std::path::Path::new(&home).join(".cache").join("kleos");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("policy.json"))
}

/// Reads fresh mandatory policy text from the local cache if it is still valid.
fn read_policy_cache() -> Option<String> {
    let path = policy_cache_path()?;
    let meta = std::fs::metadata(&path).ok()?;
    let modified = meta.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(modified).ok()?;
    if age.as_secs() > POLICY_CACHE_TTL_SECS {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("rules")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
}

/// Writes mandatory policy text to the local hook policy cache.
fn write_policy_cache(rules: &str) {
    if let Some(path) = policy_cache_path() {
        let v = serde_json::json!({ "rules": rules });
        let _ = std::fs::write(path, serde_json::to_vec(&v).unwrap_or_default());
    }
}

// --- Helpers ---

fn read_stdin_json() -> Value {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
    serde_json::from_str(&buf).unwrap_or(Value::Null)
}

/// Extracts Claude's session id from hook input or falls back to the parent process id.
fn extract_session_id(input: &Value) -> String {
    input
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| std::env::var("PPID").unwrap_or_else(|_| "unknown".to_string()))
}

/// Resolves the agent identity for hook reporting and living-context generation.
///
/// Prefers the `KLEOS_AGENT_LABEL` env var, which each harness sets to identify
/// itself ("codex" for Codex, "claude-code" for Claude Code). Falls back to
/// "claude-code" -- the historical default -- when the env var is unset, so
/// existing Claude Code sessions are unaffected. This is what stops the living
/// context from hardcoding "You are claude-code" inside Codex sessions.
fn resolve_agent() -> String {
    std::env::var("KLEOS_AGENT_LABEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "claude-code".to_string())
}

/// Emits Claude hook JSON on stdout.
fn emit(v: &Value) {
    println!("{}", serde_json::to_string(v).unwrap_or_default());
}

/// Builds a hook response that injects additional context for the current event.
fn build_context_output(event: &str, context: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": event,
            "additionalContext": context
        }
    })
}

/// POSTs JSON to the optional local sidecar and returns the parsed response on success.
async fn sidecar_post(path: &str, body: &Value, timeout: Duration) -> Option<Value> {
    let base =
        std::env::var("KLEOS_SIDECAR_URL").unwrap_or_else(|_| "http://127.0.0.1:7711".to_string());
    let url = format!("{}{}", base, path);
    let debug = std::env::var("KLEOS_HOOK_DEBUG")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let client = reqwest::Client::new();
    let mut req = client.post(&url).json(body).timeout(timeout);
    if let Ok(token) = std::env::var("KLEOS_SIDECAR_TOKEN") {
        req = req.header("Authorization", format!("Bearer {}", token));
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
        Ok(resp) => {
            if debug {
                eprintln!("[kleos-hook] sidecar {} returned {}", path, resp.status());
            }
            None
        }
        Err(e) => {
            if debug {
                eprintln!("[kleos-hook] sidecar {} failed: {}", path, e);
            }
            None
        }
    }
}

/// Converts hook tool output into bounded text for sidecar observation storage.
fn extract_tool_result_text(input: &Value, max_chars: usize) -> String {
    let raw = input
        .get("tool_result")
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| {
            input
                .get("tool_result")
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default()
        });
    raw.chars().take(max_chars).collect()
}

/// Builds a hook response that denies the current tool use with a reason.
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
        _ => format!(
            "{}: {}",
            tool_name,
            serde_json::to_string(tool_input).unwrap_or_default()
        ),
    }
}

// --- Hook handlers ---

async fn handle_session_start(client: &Client) {
    let agent = resolve_agent();

    // Register session with activity (best-effort)
    let _ = client
        .post_with_timeout(
            "/activity",
            json!({
                "agent": agent.clone(),
                "action": "session.start",
                "summary": "session started",
                "project": "unknown"
            }),
            DEFAULT_TIMEOUT,
        )
        .await;

    // Fetch growth context (best-effort)
    let growth_path = format!(
        "/growth/materialize?service={}&limit=30&max_bytes=16000",
        utf8_percent_encode(&agent, NON_ALPHANUMERIC)
    );
    let growth_text = match client.get_with_timeout(&growth_path, DEFAULT_TIMEOUT).await {
        Ok(v) => v
            .get("context")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => String::new(),
    };

    // Living prompt: the brain-aware context built by build_living_prompt on the
    // server. This is the primary content -- the Gemini hook already uses this path;
    // the Claude hook previously only carried policy rules + growth, leaving the
    // block empty whenever the operator had no mandatory rules configured.
    let living_text = match client
        .post_with_timeout(
            "/prompt/generate",
            json!({
                "agent": agent,
                "task": "session-bootstrap agent-rules infrastructure active-tasks recent-decisions",
                "include_brain": true,
                "include_growth": true,
                "include_personality": true,
            }),
            DEFAULT_TIMEOUT,
        )
        .await
    {
        Ok(v) => v
            .get("prompt")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string(),
        Err(_) => String::new(),
    };

    let rules = fetch_mandatory_rules(client).await;
    let mut ctx = String::from("=== EIDOLON LIVING CONTEXT ===\n\n");
    if !living_text.is_empty() {
        ctx.push_str(&living_text);
    }
    if !rules.is_empty() {
        ctx.push_str("\n\n--- Mandatory Rules ---\n");
        ctx.push_str(&rules);
    }
    if !growth_text.is_empty() {
        ctx.push_str("\n\n--- Growth Context ---\n");
        ctx.push_str(&growth_text);
    }
    ctx.push_str("\n\n=== END EIDOLON CONTEXT ===");

    emit(&build_context_output("SessionStart", &ctx));
}

/// Handles UserPromptSubmit by recalling context and enforcing supervisor injections.
async fn handle_user_prompt(client: &Client, input: &Value) {
    let session_id = extract_session_id(input);

    // Recall relevant memories from the sidecar before the prompt is processed.
    let recall_context = match input
        .get("prompt")
        .and_then(|v| v.as_str())
        .filter(|p| !p.is_empty())
    {
        Some(user_message) => {
            let budget = std::env::var("KLEOS_RECALL_BUDGET").unwrap_or_else(|_| "mid".to_string());
            let max_tokens: usize = std::env::var("KLEOS_RECALL_MAX_TOKENS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1024);
            let context_turns: usize = std::env::var("KLEOS_RECALL_CONTEXT_TURNS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let max_query_chars: usize = std::env::var("KLEOS_RECALL_MAX_QUERY_CHARS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(800);

            let recall_body = json!({
                "message": user_message,
                "budget": budget,
                "context_turns": context_turns,
                "max_tokens": max_tokens,
                "max_query_chars": max_query_chars,
                "session_id": session_id,
            });

            sidecar_post("/recall", &recall_body, SIDECAR_RECALL_TIMEOUT)
                .await
                .and_then(|resp| {
                    resp.get("context")
                        .and_then(|c| c.as_str())
                        .filter(|ctx| !ctx.is_empty())
                        .map(ToOwned::to_owned)
                })
        }
        None => None,
    };

    // Drain supervisor for pending violations
    let encoded_session = utf8_percent_encode(&session_id, NON_ALPHANUMERIC).to_string();
    let pending_path = format!("/supervisor/pending?session_id={}", encoded_session);
    if let Ok(v) = client
        .get_with_timeout(&pending_path, DEFAULT_TIMEOUT)
        .await
    {
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

    if let Some(context) = recall_context {
        emit(&build_context_output("UserPromptSubmit", &context));
    }
}

/// Handles Stop by recording session end and notifying the optional sidecar.
async fn handle_stop(client: &Client, input: &Value) {
    let _ = client
        .post_with_timeout(
            "/activity",
            json!({
                "agent": resolve_agent(),
                "action": "session.end",
                "summary": "session ended"
            }),
            DEFAULT_TIMEOUT,
        )
        .await;

    let session_id = extract_session_id(input);
    let _ = sidecar_post(
        "/end",
        &json!({ "session_id": session_id }),
        SIDECAR_END_TIMEOUT,
    )
    .await;
}

/// Handles PreToolUse by asking the server gate whether the proposed tool use is allowed.
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
    });

    let result = match client
        .post_with_timeout("/gate/check", gate_body, GATE_TIMEOUT)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("kleos hook pre-tool: gate unreachable ({}), allowing", e);
            return; // Fail open
        }
    };

    let allowed = result
        .get("allowed")
        .and_then(|a| a.as_bool())
        .unwrap_or(true);
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

/// Handles PostToolUse by reporting completion and forwarding an optional observation.
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
                "agent": resolve_agent(),
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

    let observe_body = json!({
        "tool_name": tool_name,
        "content": extract_tool_result_text(input, 1500),
        "role": "tool",
        "session_id": session_id,
        "importance": 3,
        "category": "discovery",
    });
    let _ = sidecar_post("/observe", &observe_body, SIDECAR_OBSERVE_TIMEOUT).await;
}

// --- Entry point ---

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
            let input = read_stdin_json();
            handle_stop(client, &input).await;
        }
        HookCommands::PreTool => {
            let input = read_stdin_json();
            handle_pre_tool(client, &input).await;
        }
        HookCommands::PostTool | HookCommands::PostBash => {
            let input = read_stdin_json();
            handle_post_tool(client, &input).await;
        }
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// Verifies context hook output uses Claude's hookSpecificOutput shape.
    fn test_build_context_output_structure() {
        let out = build_context_output("SessionStart", "some context");
        let inner = &out["hookSpecificOutput"];
        assert_eq!(inner["hookEventName"], "SessionStart");
        assert_eq!(inner["additionalContext"], "some context");
        assert!(inner.get("permissionDecision").is_none());
    }

    #[test]
    /// Verifies deny hook output carries the permission decision and reason.
    fn test_build_deny_output_structure() {
        let out = build_deny_output("PreToolUse", "blocked!");
        let inner = &out["hookSpecificOutput"];
        assert_eq!(inner["hookEventName"], "PreToolUse");
        assert_eq!(inner["permissionDecision"], "deny");
        assert_eq!(inner["permissionDecisionReason"], "blocked!");
    }

    #[test]
    /// Verifies explicit session ids are preserved from hook input.
    fn test_extract_session_id_present() {
        let input = json!({ "session_id": "abc-123" });
        assert_eq!(extract_session_id(&input), "abc-123");
    }

    #[test]
    /// Verifies session id extraction still returns a non-empty fallback.
    fn test_extract_session_id_fallback() {
        let input = json!({});
        let id = extract_session_id(&input);
        assert!(!id.is_empty());
    }

    #[test]
    /// Verifies Bash tool inputs use the literal command string.
    fn test_derive_command_bash() {
        let input = json!({"command": "ls -la"});
        assert_eq!(derive_command("Bash", &input), "ls -la");
    }

    #[test]
    /// Verifies Write tool inputs summarize the destination path.
    fn test_derive_command_write() {
        let input = json!({"file_path": "/tmp/foo.rs"});
        assert_eq!(derive_command("Write", &input), "Write /tmp/foo.rs");
    }

    #[test]
    /// Verifies Edit tool inputs summarize the edited path.
    fn test_derive_command_edit() {
        let input = json!({"file_path": "/tmp/bar.rs"});
        assert_eq!(derive_command("Edit", &input), "Edit /tmp/bar.rs");
    }

    #[test]
    /// Verifies WebFetch inputs derive a useful URL command summary.
    fn test_derive_command_other() {
        let input = json!({"url": "https://example.com"});
        let cmd = derive_command("WebFetch", &input);
        assert_eq!(cmd, "https://example.com");
    }
}
