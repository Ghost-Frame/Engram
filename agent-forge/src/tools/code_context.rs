//! Agent-Forge tool adapters for the persistent local code index.

use crate::code_context::{CodeIndex, ContextQuery, RelationQuery};
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};

/// Refresh and retrieve a bounded, model-ready source context pack.
pub fn code_context(_db: &crate::db::Database, input: ContextQuery) -> ToolResult {
    let index = CodeIndex::open_default().map_err(index_error)?;
    let pack = index.context(&input).map_err(index_error)?;
    let rendered = pack.render();
    let mut output = Output::ok(format!(
        "Selected {} code snippets at index revision {}",
        pack.snippets.len(),
        pack.index_revision
    ));
    output.data = Some(serde_json::json!({
        "context": rendered,
        "pack": pack,
    }));
    Ok(output)
}

/// Refresh and retrieve bounded structural relations for one symbol or path.
pub fn code_relations(_db: &crate::db::Database, input: RelationQuery) -> ToolResult {
    let index = CodeIndex::open_default().map_err(index_error)?;
    let relations = index.relations(&input).map_err(index_error)?;
    let mut output = Output::ok(format!("Found {} code relations", relations.len()));
    output.data = Some(serde_json::json!({"relations": relations}));
    Ok(output)
}

/// Translate index failures into the existing Agent-Forge tool error envelope.
fn index_error(error: crate::code_context::IndexError) -> ToolError {
    ToolError::DatabaseError(error.to_string())
}
