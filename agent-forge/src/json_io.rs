use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IoError {
    #[error("Failed to read input file: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse JSON: {0}")]
    ParseError(#[from] serde_json::Error),
}

#[derive(Serialize)]
pub struct Output {
    pub success: bool,
    pub id: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl Output {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            id: None,
            message: message.into(),
            data: None,
        }
    }

    pub fn ok_with_id(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            success: true,
            id: Some(id.into()),
            message: message.into(),
            data: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            id: None,
            message: message.into(),
            data: None,
        }
    }
}

pub fn read_input<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, IoError> {
    let content = fs::read_to_string(path)?;
    let parsed = serde_json::from_str(&content)?;
    Ok(parsed)
}

pub fn write_output(path: &Path, output: &Output) -> Result<(), IoError> {
    let content = serde_json::to_string_pretty(output)?;
    fs::write(path, content)?;
    Ok(())
}
