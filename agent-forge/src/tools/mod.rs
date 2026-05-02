pub mod ast;
pub mod hypothesis;
pub mod session;
pub mod spec;
pub mod think;
pub mod verify;

use crate::json_io::Output;

pub type ToolResult = Result<Output, ToolError>;

#[derive(Debug)]
pub enum ToolError {
    MissingField(String),
    InvalidValue(String),
    DatabaseError(String),
    IoError(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::MissingField(s) => write!(f, "Missing required field: {}", s),
            ToolError::InvalidValue(s) => write!(f, "Invalid value: {}", s),
            ToolError::DatabaseError(s) => write!(f, "Database error: {}", s),
            ToolError::IoError(s) => write!(f, "I/O error: {}", s),
        }
    }
}

impl std::error::Error for ToolError {}

/// Mark a forge artifact as the currently-active gate state for Claude Code's
/// enforce-agent-forge.sh hook. Best-effort: failures here must not abort the
/// caller, since the DB record (the source of truth) is already committed.
///
/// Writes ~/.claude/session-env/agent-forge-active with "<id>:<kind>".
pub fn set_session_active(id: &str, kind: &str) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    let dir = std::path::PathBuf::from(home)
        .join(".claude")
        .join("session-env");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("agent-forge-active"), format!("{}:{}", id, kind));
}
