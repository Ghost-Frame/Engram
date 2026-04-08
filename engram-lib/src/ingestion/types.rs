// ============================================================================
// Ingestion pipeline types -- ported from ingestion/types.ts
// ============================================================================

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// -- Enums --

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum IngestMode {
    Extract,
    #[default]
    Raw,
}


impl std::fmt::Display for IngestMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extract => write!(f, "extract"),
            Self::Raw => write!(f, "raw"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SupportedFormat {
    Markdown,
    Html,
    Pdf,
    Docx,
    Csv,
    Jsonl,
    #[serde(rename = "claude-export")]
    ClaudeExport,
    #[serde(rename = "chatgpt-export")]
    ChatGptExport,
    Messages,
    Zip,
    #[serde(rename = "plaintext")]
    PlainText,
}

impl std::fmt::Display for SupportedFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Markdown => write!(f, "markdown"),
            Self::Html => write!(f, "html"),
            Self::Pdf => write!(f, "pdf"),
            Self::Docx => write!(f, "docx"),
            Self::Csv => write!(f, "csv"),
            Self::Jsonl => write!(f, "jsonl"),
            Self::ClaudeExport => write!(f, "claude-export"),
            Self::ChatGptExport => write!(f, "chatgpt-export"),
            Self::Messages => write!(f, "messages"),
            Self::Zip => write!(f, "zip"),
            Self::PlainText => write!(f, "plaintext"),
        }
    }
}

impl std::str::FromStr for SupportedFormat {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "markdown" => Ok(Self::Markdown),
            "html" => Ok(Self::Html),
            "pdf" => Ok(Self::Pdf),
            "docx" => Ok(Self::Docx),
            "csv" => Ok(Self::Csv),
            "jsonl" => Ok(Self::Jsonl),
            "claude-export" => Ok(Self::ClaudeExport),
            "chatgpt-export" => Ok(Self::ChatGptExport),
            "messages" => Ok(Self::Messages),
            "zip" => Ok(Self::Zip),
            "plaintext" => Ok(Self::PlainText),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown format: {}",
                s
            ))),
        }
    }
}

/// Supported file extensions for ingestion.
pub const SUPPORTED_EXTENSIONS: &[&str] = &[
    ".md", ".txt", ".text", ".html", ".htm", ".pdf",
    ".docx", ".csv", ".jsonl", ".json", ".zip",
];

// -- Core document types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDocument {
    pub title: String,
    pub text: String,
    pub metadata: HashMap<String, serde_json::Value>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub text: String,
    pub index: usize,
    pub total: usize,
    pub document_title: String,
    pub source: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

// -- Options and results --

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct ChunkerOptions {
    /// Maximum characters per chunk (default: 3000)
    pub max_chunk_size: Option<usize>,
    /// Overlap between consecutive chunks (default: 200)
    pub overlap: Option<usize>,
    /// Respect heading/paragraph/sentence boundaries (default: true)
    pub respect_structure: Option<bool>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOptions {
    pub source: String,
    pub category: String,
    pub user_id: i64,
    pub space_id: Option<i64>,
    pub project_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub entity_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessResult {
    pub memories_created: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestOptions {
    pub mode: IngestMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<SupportedFormat>,
    pub source: String,
    pub category: String,
    pub user_id: i64,
    pub space_id: Option<i64>,
    pub project_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub entity_ids: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunker_options: Option<ChunkerOptions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub job_id: String,
    pub status: IngestStatus,
    pub total_documents: usize,
    pub total_chunks: usize,
    pub total_memories: usize,
    pub errors: Vec<String>,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngestStatus {
    Processing,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestProgress {
    pub job_id: String,
    pub chunks_done: usize,
    pub chunks_total: usize,
    pub memories_created: usize,
    pub current_file: Option<String>,
}

/// Metadata hints for format detection.
#[derive(Debug, Clone, Default)]
pub struct FormatMeta {
    pub extension: Option<String>,
    pub mime: Option<String>,
}
