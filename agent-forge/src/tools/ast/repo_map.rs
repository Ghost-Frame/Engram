//! Persistent repository map generator backed by the shared local code index.

use crate::code_context::CodeIndex;
use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use serde::Deserialize;

/// Input for `repo_map`: the directory root to scan, path fragments to
/// prioritise, and a token budget that caps the output size.
#[derive(Deserialize)]
pub struct RepoMapInput {
    pub path: Option<String>,
    pub focus: Option<Vec<String>>,
    pub max_tokens: Option<usize>,
}

/// Refresh the repository incrementally and format indexed symbols within the budget.
pub fn repo_map(_db: &Database, input: RepoMapInput) -> ToolResult {
    let path = input
        .path
        .ok_or_else(|| ToolError::MissingField("path".into()))?;

    let max_tokens = input.max_tokens.unwrap_or(4000);
    let focus = input.focus.unwrap_or_default();

    let index = CodeIndex::open_default().map_err(index_error)?;
    let (refresh, symbols) = index
        .repository_symbols(&path, &focus, max_tokens)
        .map_err(index_error)?;
    let output_lines: Vec<String> = symbols
        .iter()
        .map(|symbol| {
            format!(
                "{} {} ({}:{})",
                symbol.kind, symbol.name, symbol.path, symbol.line
            )
        })
        .collect();

    let mut result = Output::ok(format!(
        "Mapped {} files, {} symbols (top {} shown)",
        refresh.files_seen,
        output_lines.len(),
        output_lines.len()
    ));

    result.data = Some(serde_json::json!({
        "files_scanned": refresh.files_seen,
        "symbols_shown": output_lines.len(),
        "map": output_lines.join("\n"),
        "index_revision": refresh.index_revision,
    }));

    Ok(result)
}

/// Translate local index failures into the standard tool error envelope.
fn index_error(error: crate::code_context::IndexError) -> ToolError {
    ToolError::DatabaseError(error.to_string())
}
