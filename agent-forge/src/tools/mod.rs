pub mod hypothesis;
pub mod session;
pub mod spec;
pub mod think;
pub mod verify;

use crate::db::Database;
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
