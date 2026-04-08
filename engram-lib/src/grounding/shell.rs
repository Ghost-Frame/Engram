// GROUNDING SHELL - Shell backend (ported from TS grounding/backends/shell.ts)
use super::types::*;
use serde_json::json;
use std::collections::HashMap;
use std::time::Instant;
use tokio::process::Command;

const DEFAULT_TIMEOUT_MS: u64 = 30000;
const MAX_OUTPUT_SIZE: usize = 100_000;

pub fn shell_tools() -> Vec<ToolSchema> {
    vec![
        ToolSchema { name: "shell_exec".into(), description: "Execute a shell command".into(), parameters: json!({"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string"}},"required":["command"]}), return_schema: None, usage_hint: None, latency_hint: None, backend_type: BackendType::Shell, security_policy: None },
        ToolSchema { name: "file_read".into(), description: "Read a file".into(), parameters: json!({"type":"object","properties":{"path":{"type":"string"},"max_lines":{"type":"number"}},"required":["path"]}), return_schema: None, usage_hint: None, latency_hint: None, backend_type: BackendType::Shell, security_policy: None },
        ToolSchema { name: "file_list".into(), description: "List directory".into(), parameters: json!({"type":"object","properties":{"path":{"type":"string"},"recursive":{"type":"boolean"}},"required":["path"]}), return_schema: None, usage_hint: None, latency_hint: None, backend_type: BackendType::Shell, security_policy: None },
        ToolSchema { name: "system_info".into(), description: "System info".into(), parameters: json!({"type":"object","properties":{}}), return_schema: None, usage_hint: None, latency_hint: None, backend_type: BackendType::Shell, security_policy: None },
    ]
}

pub struct ShellProvider { pub name: String, sessions: HashMap<String, SessionInfo> }
impl ShellProvider {
    pub fn new(name: &str) -> Self { Self { name: name.to_string(), sessions: HashMap::new() } }

    pub async fn execute_tool(&self, tool_name: &str, args: &serde_json::Value, timeout_ms: Option<u64>) -> ToolResult {
        let t = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        match tool_name {
            "shell_exec" => self.exec_shell(args, t).await,
            "file_read" => self.read_file(args).await,
            "file_list" => self.list_files(args, t).await,
            "system_info" => self.system_info(t).await,
            _ => ToolResult { status: ToolStatus::Error, content: json!(null), error: Some(format!("Unknown tool: {}", tool_name)), execution_time_ms: None },
        }
    }

    async fn exec_shell(&self, args: &serde_json::Value, timeout_ms: u64) -> ToolResult {
        let cmd_str = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if cmd_str.is_empty() { return ToolResult { status: ToolStatus::Error, content: json!(null), error: Some("command required".into()), execution_time_ms: None }; }
        let start = Instant::now();
        let (shell, flag) = if cfg!(target_os = "windows") { ("cmd", "/C") } else { ("sh", "-c") };
        let mut cmd = Command::new(shell); cmd.arg(flag).arg(cmd_str);
        if let Some(cwd) = args.get("cwd").and_then(|v| v.as_str()) { cmd.current_dir(cwd); }
        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), cmd.output()).await {
            Ok(Ok(out)) => {
                let so = String::from_utf8_lossy(&out.stdout); let se = String::from_utf8_lossy(&out.stderr);
                let s = &so[..so.len().min(MAX_OUTPUT_SIZE)]; let e = &se[..se.len().min(MAX_OUTPUT_SIZE)];
                ToolResult { status: if out.status.success() { ToolStatus::Success } else { ToolStatus::Error }, content: json!({"stdout": s, "stderr": e}), error: if out.status.success() { None } else { Some(format!("exit {:?}", out.status.code())) }, execution_time_ms: Some(start.elapsed().as_millis() as u64) }
            }
            Ok(Err(e)) => ToolResult { status: ToolStatus::Error, content: json!(null), error: Some(e.to_string()), execution_time_ms: Some(start.elapsed().as_millis() as u64) },
            Err(_) => ToolResult { status: ToolStatus::Error, content: json!(null), error: Some("timed out".into()), execution_time_ms: Some(timeout_ms) },
        }
    }

    async fn read_file(&self, args: &serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() { return ToolResult { status: ToolStatus::Error, content: json!(null), error: Some("path required".into()), execution_time_ms: None }; }
        match tokio::fs::read_to_string(path).await {
            Ok(mut content) => {
                if let Some(max) = args.get("max_lines").and_then(|v| v.as_u64()) { let lines: Vec<&str> = content.lines().take(max as usize).collect(); content = lines.join("\n"); }
                content.truncate(MAX_OUTPUT_SIZE);
                ToolResult { status: ToolStatus::Success, content: json!(content), error: None, execution_time_ms: None }
            }
            Err(e) => ToolResult { status: ToolStatus::Error, content: json!(null), error: Some(e.to_string()), execution_time_ms: None },
        }
    }

    async fn list_files(&self, args: &serde_json::Value, timeout: u64) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let recursive = args.get("recursive").and_then(|v| v.as_bool()).unwrap_or(false);
        let cmd = if cfg!(target_os = "windows") { if recursive { format!("dir /s /b {}", path) } else { format!("dir /b {}", path) } } else { if recursive { format!("find {} -type f", path) } else { format!("ls -la {}", path) } };
        self.exec_shell(&json!({"command": cmd}), timeout).await
    }

    async fn system_info(&self, timeout: u64) -> ToolResult {
        let cmd = if cfg!(target_os = "windows") { "systeminfo" } else { "uname -a" };
        self.exec_shell(&json!({"command": cmd}), timeout).await
    }

    pub fn create_session(&mut self, config: &SessionConfig) -> SessionInfo {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let info = SessionInfo { id: id.clone(), name: config.name.clone(), backend: BackendType::Shell, status: SessionStatus::Connected, tools: shell_tools().iter().map(|t| t.name.clone()).collect(), created_at: now.clone(), last_activity_at: now, metadata: config.metadata.clone() };
        self.sessions.insert(id, info.clone());
        info
    }
    pub fn destroy_session(&mut self, id: &str) { self.sessions.remove(id); }
    pub fn list_sessions(&self) -> Vec<&SessionInfo> { self.sessions.values().collect() }
}
