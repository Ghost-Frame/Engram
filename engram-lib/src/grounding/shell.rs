// GROUNDING SHELL - Shell backend (ported from TS grounding/backends/shell.ts)
use super::types::*;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::process::Command;

const DEFAULT_TIMEOUT_MS: u64 = 30000;
const MAX_OUTPUT_SIZE: usize = 100_000;

/// SECURITY (SEC-CRIT-4 / SEC-HIGH-7): comma-separated allowlist of argv0
/// values that `shell_exec` will accept. Empty or unset means "no shell
/// execution allowed at all." Each entry is compared exactly to the first
/// token of the parsed command line.
const ENV_ALLOWED_CMDS: &str = "ENGRAM_GROUNDING_ALLOWED_CMDS";

/// SECURITY (SEC-CRIT-4 / SEC-HIGH-7): base directory that ALL file and
/// list operations must resolve inside after `canonicalize`. Unset means
/// "no file operations allowed at all."
const ENV_BASE_DIR: &str = "ENGRAM_GROUNDING_BASE_DIR";

/// Shell metacharacters that are never allowed in a shell_exec command.
/// Blocking these keeps the parser honest: any allowed invocation is a
/// pure `argv` form with no redirection, no chaining, no subshells.
const FORBIDDEN_META: &[char] = &[
    ';', '|', '&', '`', '$', '>', '<', '\n', '\r', '\\', '(', ')', '{', '}',
];

fn shell_error(msg: impl Into<String>) -> ToolResult {
    ToolResult {
        status: ToolStatus::Error,
        content: json!(null),
        error: Some(msg.into()),
        execution_time_ms: None,
    }
}

fn load_allowed_cmds() -> Vec<String> {
    std::env::var(ENV_ALLOWED_CMDS)
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn load_base_dir() -> Option<PathBuf> {
    let raw = std::env::var(ENV_BASE_DIR).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    // `canonicalize` here turns the configured base dir into the same
    // absolute prefix we will compare caller-supplied paths against.
    std::fs::canonicalize(raw).ok()
}

/// Split a command string into argv tokens honoring single and double
/// quotes, with no glob/brace/variable expansion. Returns Err on
/// unterminated quotes. This is deliberately tiny: anything complex
/// should go through a native Rust API, not this function.
fn split_argv(cmd: &str) -> Result<Vec<String>, &'static str> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut saw = false;
    for c in cmd.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
            }
            (Some(_), c) => {
                cur.push(c);
            }
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                saw = true;
            }
            (None, c) if c.is_whitespace() => {
                if saw {
                    out.push(std::mem::take(&mut cur));
                    saw = false;
                }
            }
            (None, c) => {
                cur.push(c);
                saw = true;
            }
        }
    }
    if quote.is_some() {
        return Err("unterminated quote in command");
    }
    if saw {
        out.push(cur);
    }
    Ok(out)
}

/// Resolve `path` against the configured base directory. The final
/// canonicalized absolute path must start with the base. Returns an
/// error on traversal, missing base dir, or unresolvable path.
fn confine_path(path: &str) -> Result<PathBuf, String> {
    let base = load_base_dir()
        .ok_or_else(|| format!("{} not configured; file operations denied", ENV_BASE_DIR))?;
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    let canonical = std::fs::canonicalize(&joined)
        .map_err(|e| format!("path cannot be resolved: {}", e))?;
    if !canonical.starts_with(&base) {
        return Err(format!(
            "path {} escapes base directory {}",
            canonical.display(),
            base.display()
        ));
    }
    Ok(canonical)
}

pub fn shell_tools() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "shell_exec".into(),
            description: "Execute a shell command".into(),
            parameters: json!({"type":"object","properties":{"command":{"type":"string"},"cwd":{"type":"string"}},"required":["command"]}),
            return_schema: None,
            usage_hint: None,
            latency_hint: None,
            backend_type: BackendType::Shell,
            security_policy: None,
        },
        ToolSchema {
            name: "file_read".into(),
            description: "Read a file".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"},"max_lines":{"type":"number"}},"required":["path"]}),
            return_schema: None,
            usage_hint: None,
            latency_hint: None,
            backend_type: BackendType::Shell,
            security_policy: None,
        },
        ToolSchema {
            name: "file_list".into(),
            description: "List directory".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"},"recursive":{"type":"boolean"}},"required":["path"]}),
            return_schema: None,
            usage_hint: None,
            latency_hint: None,
            backend_type: BackendType::Shell,
            security_policy: None,
        },
        ToolSchema {
            name: "git_status".into(),
            description: "Get git status for a repository".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
            return_schema: None,
            usage_hint: None,
            latency_hint: None,
            backend_type: BackendType::Shell,
            security_policy: None,
        },
        ToolSchema {
            name: "system_info".into(),
            description: "System info".into(),
            parameters: json!({"type":"object","properties":{}}),
            return_schema: None,
            usage_hint: None,
            latency_hint: None,
            backend_type: BackendType::Shell,
            security_policy: None,
        },
    ]
}

pub struct ShellProvider {
    pub name: String,
    sessions: HashMap<String, SessionInfo>,
}
impl ShellProvider {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            sessions: HashMap::new(),
        }
    }

    pub async fn execute_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        timeout_ms: Option<u64>,
    ) -> ToolResult {
        let t = timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS);
        match tool_name {
            "shell_exec" => self.exec_shell(args, t).await,
            "file_read" => self.read_file(args).await,
            "file_list" => self.list_files(args, t).await,
            "git_status" => self.git_status(args, t).await,
            "system_info" => self.system_info(t).await,
            _ => ToolResult {
                status: ToolStatus::Error,
                content: json!(null),
                error: Some(format!("Unknown tool: {}", tool_name)),
                execution_time_ms: None,
            },
        }
    }

    async fn exec_shell(&self, args: &serde_json::Value, timeout_ms: u64) -> ToolResult {
        let cmd_str = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
        if cmd_str.is_empty() {
            return shell_error("command required");
        }

        // SECURITY (SEC-CRIT-4): reject any shell metacharacter. This
        // collapses the attack surface to a pure `argv` invocation with
        // no chaining, redirection, subshell, or variable expansion.
        if let Some(bad) = cmd_str.chars().find(|c| FORBIDDEN_META.contains(c)) {
            return shell_error(format!(
                "shell metacharacter {:?} is not allowed in grounding shell_exec",
                bad
            ));
        }

        let argv = match split_argv(cmd_str) {
            Ok(v) if !v.is_empty() => v,
            Ok(_) => return shell_error("empty command"),
            Err(e) => return shell_error(e),
        };

        let allowlist = load_allowed_cmds();
        if allowlist.is_empty() {
            return shell_error(format!(
                "{} is empty or unset; grounding shell_exec is disabled",
                ENV_ALLOWED_CMDS
            ));
        }
        if !allowlist.iter().any(|a| a == &argv[0]) {
            return shell_error(format!(
                "command {:?} is not in {}",
                argv[0], ENV_ALLOWED_CMDS
            ));
        }

        // cwd is optional; if present it must also confine inside the
        // configured base dir.
        let cwd_resolved = match args.get("cwd").and_then(|v| v.as_str()) {
            Some(cwd) => match confine_path(cwd) {
                Ok(p) => Some(p),
                Err(e) => return shell_error(e),
            },
            None => None,
        };

        let start = Instant::now();
        let mut cmd = Command::new(&argv[0]);
        for a in &argv[1..] {
            cmd.arg(a);
        }
        if let Some(p) = cwd_resolved {
            cmd.current_dir(p);
        }
        // Drop inherited env so the subprocess can't read ENGRAM_* or
        // ENGRAM_BOOTSTRAP_SECRET from the parent process.
        cmd.env_clear();
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }

        match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), cmd.output()).await
        {
            Ok(Ok(out)) => {
                let so = String::from_utf8_lossy(&out.stdout);
                let se = String::from_utf8_lossy(&out.stderr);
                let s = &so[..so.len().min(MAX_OUTPUT_SIZE)];
                let e = &se[..se.len().min(MAX_OUTPUT_SIZE)];
                ToolResult {
                    status: if out.status.success() {
                        ToolStatus::Success
                    } else {
                        ToolStatus::Error
                    },
                    content: json!({"stdout": s, "stderr": e}),
                    error: if out.status.success() {
                        None
                    } else {
                        Some(format!("exit {:?}", out.status.code()))
                    },
                    execution_time_ms: Some(start.elapsed().as_millis() as u64),
                }
            }
            Ok(Err(e)) => ToolResult {
                status: ToolStatus::Error,
                content: json!(null),
                error: Some(e.to_string()),
                execution_time_ms: Some(start.elapsed().as_millis() as u64),
            },
            Err(_) => ToolResult {
                status: ToolStatus::Error,
                content: json!(null),
                error: Some("timed out".into()),
                execution_time_ms: Some(timeout_ms),
            },
        }
    }

    async fn read_file(&self, args: &serde_json::Value) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if path.is_empty() {
            return shell_error("path required");
        }
        // SECURITY (SEC-CRIT-4 / SEC-HIGH-7): canonicalize and confine.
        let resolved = match confine_path(path) {
            Ok(p) => p,
            Err(e) => return shell_error(e),
        };
        match tokio::fs::read_to_string(&resolved).await {
            Ok(mut content) => {
                if let Some(max) = args.get("max_lines").and_then(|v| v.as_u64()) {
                    let lines: Vec<&str> = content.lines().take(max as usize).collect();
                    content = lines.join("\n");
                }
                if content.len() > MAX_OUTPUT_SIZE {
                    content.truncate(MAX_OUTPUT_SIZE);
                }
                ToolResult {
                    status: ToolStatus::Success,
                    content: json!(content),
                    error: None,
                    execution_time_ms: None,
                }
            }
            Err(_) => shell_error("read failed"),
        }
    }

    async fn list_files(&self, args: &serde_json::Value, _timeout: u64) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let recursive = args
            .get("recursive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // SECURITY (SEC-CRIT-4): confine to base dir, then walk via
        // std::fs so we never touch a shell.
        let resolved = match confine_path(path) {
            Ok(p) => p,
            Err(e) => return shell_error(e),
        };
        let base = match load_base_dir() {
            Some(b) => b,
            None => return shell_error(format!("{} not configured", ENV_BASE_DIR)),
        };
        let mut out = Vec::new();
        if recursive {
            fn walk(root: &Path, base: &Path, out: &mut Vec<String>, cap: usize) {
                if out.len() >= cap {
                    return;
                }
                let Ok(entries) = std::fs::read_dir(root) else {
                    return;
                };
                for entry in entries.flatten() {
                    let p = entry.path();
                    if let Ok(canon) = std::fs::canonicalize(&p) {
                        if !canon.starts_with(base) {
                            continue;
                        }
                        if canon.is_dir() {
                            walk(&canon, base, out, cap);
                        } else if let Ok(rel) = canon.strip_prefix(base) {
                            out.push(rel.to_string_lossy().into_owned());
                            if out.len() >= cap {
                                return;
                            }
                        }
                    }
                }
            }
            walk(&resolved, &base, &mut out, 10_000);
        } else {
            if let Ok(entries) = std::fs::read_dir(&resolved) {
                for entry in entries.flatten() {
                    out.push(entry.file_name().to_string_lossy().into_owned());
                    if out.len() >= 10_000 {
                        break;
                    }
                }
            }
        }
        ToolResult {
            status: ToolStatus::Success,
            content: json!(out),
            error: None,
            execution_time_ms: None,
        }
    }

    async fn git_status(&self, args: &serde_json::Value, timeout: u64) -> ToolResult {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        // `git` must still appear in the allowlist or exec_shell will
        // refuse; the path is confined to the base dir before being
        // accepted as cwd.
        self.exec_shell(
            &json!({
                "command": "git status --porcelain",
                "cwd": path,
            }),
            timeout,
        )
        .await
    }

    async fn system_info(&self, timeout: u64) -> ToolResult {
        let cmd = if cfg!(target_os = "windows") {
            "systeminfo"
        } else {
            "uname -a"
        };
        self.exec_shell(&json!({"command": cmd}), timeout).await
    }

    pub fn create_session(&mut self, config: &SessionConfig) -> SessionInfo {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let info = SessionInfo {
            id: id.clone(),
            name: config.name.clone(),
            backend: BackendType::Shell,
            status: SessionStatus::Connected,
            tools: shell_tools().iter().map(|t| t.name.clone()).collect(),
            created_at: now.clone(),
            last_activity_at: now,
            metadata: config.metadata.clone(),
        };
        self.sessions.insert(id, info.clone());
        info
    }
    pub fn destroy_session(&mut self, id: &str) {
        self.sessions.remove(id);
    }
    pub fn list_sessions(&self) -> Vec<&SessionInfo> {
        self.sessions.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_argv_basic() {
        assert_eq!(
            split_argv("git status --porcelain").unwrap(),
            vec!["git", "status", "--porcelain"]
        );
    }

    #[test]
    fn split_argv_quotes() {
        assert_eq!(
            split_argv("echo 'hello world' bye").unwrap(),
            vec!["echo", "hello world", "bye"]
        );
    }

    #[test]
    fn split_argv_unterminated() {
        assert!(split_argv("echo 'oops").is_err());
    }

    #[test]
    fn metacharacters_rejected() {
        for bad in &[";", "|", "&", "`", "$(ls)", ">", "<", "`ls`"] {
            assert!(
                FORBIDDEN_META.iter().any(|c| bad.contains(*c)),
                "expected {:?} to contain a forbidden char",
                bad
            );
        }
    }
}
