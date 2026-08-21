//! Persistent symbol search backed by the shared local code index.

use crate::code_context::CodeIndex;
use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use serde::Deserialize;

/// Input for `search_code`: the symbol name fragment to search for, a root
/// path to walk, an optional kind filter ("function", "class", etc.), and
/// a result cap.
#[derive(Deserialize)]
pub struct SearchCodeInput {
    pub query: Option<String>,
    pub path: Option<String>,
    pub symbol_type: Option<String>,
    pub limit: Option<usize>,
}

/// One symbol match: file path, 1-based line/column, the kind (function/class/
/// enum/etc.), the symbol name, and the source line as context.
#[derive(serde::Serialize)]
struct SearchResult {
    file: String,
    line: usize,
    column: usize,
    kind: String,
    name: String,
    context: String,
}

/// Incrementally refresh and search symbol names case-insensitively.
pub fn search_code(_db: &Database, input: SearchCodeInput) -> ToolResult {
    let query = input
        .query
        .ok_or_else(|| ToolError::MissingField("query".into()))?;

    let path = input.path.unwrap_or_else(|| ".".into());
    let limit = input.limit.unwrap_or(20);
    let index = CodeIndex::open_default().map_err(index_error)?;
    let symbols = index
        .search_symbols(&path, &query, input.symbol_type.as_deref(), limit)
        .map_err(index_error)?;
    let results: Vec<SearchResult> = symbols
        .into_iter()
        .map(|symbol| SearchResult {
            file: symbol.path,
            line: symbol.line,
            column: 1,
            kind: symbol.kind,
            name: symbol.name,
            context: symbol.signature,
        })
        .collect();

    let mut output = Output::ok(format!("Found {} matches for '{}'", results.len(), query));
    output.data = Some(serde_json::json!({
        "query": query,
        "matches": results,
    }));

    Ok(output)
}

/// Translate local index failures into the standard tool error envelope.
fn index_error(error: crate::code_context::IndexError) -> ToolError {
    ToolError::DatabaseError(error.to_string())
}
