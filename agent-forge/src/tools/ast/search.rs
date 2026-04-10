use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use crate::treesitter::{is_supported_extension, parser::parse_file};
use ignore::WalkBuilder;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
pub struct SearchCodeInput {
    pub query: Option<String>,
    pub path: Option<String>,
    pub symbol_type: Option<String>,
    pub limit: Option<usize>,
}

#[derive(serde::Serialize)]
struct SearchResult {
    file: String,
    line: usize,
    column: usize,
    kind: String,
    name: String,
    context: String,
}

pub fn search_code(_db: &Database, input: SearchCodeInput) -> ToolResult {
    let query = input
        .query
        .ok_or_else(|| ToolError::MissingField("query".into()))?;

    let path = input.path.unwrap_or_else(|| ".".into());
    let limit = input.limit.unwrap_or(20);
    let type_filter = input.symbol_type;

    let root = Path::new(&path);
    if !root.exists() {
        return Err(ToolError::IoError(format!("Path does not exist: {}", path)));
    }

    let mut results: Vec<SearchResult> = Vec::new();
    let query_lower = query.to_lowercase();

    for entry in WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(|e| e.ok())
    {
        if results.len() >= limit {
            break;
        }

        let entry_path = entry.path();
        if !entry_path.is_file() {
            continue;
        }

        let ext = match entry_path.extension().and_then(|e| e.to_str()) {
            Some(e) => e,
            None => continue,
        };

        if !is_supported_extension(ext) {
            continue;
        }

        if let Some(parsed) = parse_file(entry_path) {
            search_in_tree(&parsed, &query_lower, &type_filter, &mut results, limit);
        }
    }

    let mut output = Output::ok(format!("Found {} matches for '{}'", results.len(), query));
    output.data = Some(serde_json::json!({
        "query": query,
        "matches": results,
    }));

    Ok(output)
}

fn search_in_tree(
    parsed: &crate::treesitter::parser::ParsedFile,
    query: &str,
    type_filter: &Option<String>,
    results: &mut Vec<SearchResult>,
    limit: usize,
) {
    let mut cursor = parsed.tree.walk();

    loop {
        if results.len() >= limit {
            return;
        }

        let node = cursor.node();
        let kind = node.kind();

        let symbol_kind = match kind {
            "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition" => Some("function"),
            "struct_item" | "class_definition" | "class_declaration" => Some("class"),
            "impl_item" => Some("impl"),
            "trait_item" | "interface_declaration" => Some("trait"),
            "enum_item" => Some("enum"),
            "const_item" | "static_item" | "variable_declarator" => Some("variable"),
            "use_declaration" | "import_statement" => Some("import"),
            _ => None,
        };

        if let Some(sk) = symbol_kind {
            // Check type filter
            if let Some(ref filter) = type_filter {
                if filter != "any" && filter != sk {
                    if cursor.goto_first_child() {
                        continue;
                    }
                    while !cursor.goto_next_sibling() {
                        if !cursor.goto_parent() {
                            return;
                        }
                    }
                    continue;
                }
            }

            if let Some(name_node) = node.child_by_field_name("name") {
                let name = &parsed.source[name_node.byte_range()];
                if name.to_lowercase().contains(query) {
                    let start = node.start_position();
                    let line_start = parsed.source[..node.start_byte()]
                        .rfind('\n')
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let line_end = parsed.source[node.start_byte()..]
                        .find('\n')
                        .map(|i| node.start_byte() + i)
                        .unwrap_or(parsed.source.len());
                    let context = parsed.source[line_start..line_end].trim().to_string();

                    results.push(SearchResult {
                        file: parsed.path.clone(),
                        line: start.row + 1,
                        column: start.column + 1,
                        kind: sk.to_string(),
                        name: name.to_string(),
                        context,
                    });
                }
            }
        }

        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}
