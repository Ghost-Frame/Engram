use crate::db::Database;
use crate::json_io::Output;
use crate::tools::{ToolError, ToolResult};
use crate::treesitter::{is_supported_extension, parser::parse_file};
use ignore::WalkBuilder;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
pub struct RepoMapInput {
    pub path: Option<String>,
    pub focus: Option<Vec<String>>,
    pub max_tokens: Option<usize>,
}

#[derive(Default)]
struct Symbol {
    name: String,
    kind: String,
    file: String,
    line: usize,
    references: usize,
}

pub fn repo_map(_db: &Database, input: RepoMapInput) -> ToolResult {
    let path = input
        .path
        .ok_or_else(|| ToolError::MissingField("path".into()))?;

    let max_tokens = input.max_tokens.unwrap_or(4000);
    let focus = input.focus.unwrap_or_default();

    let root = Path::new(&path);
    if !root.exists() {
        return Err(ToolError::IoError(format!("Path does not exist: {}", path)));
    }

    let mut symbols: Vec<Symbol> = Vec::new();
    let mut file_count = 0;

    // Walk directory respecting gitignore
    for entry in WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(|e| e.ok())
    {
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

        file_count += 1;

        if let Some(parsed) = parse_file(entry_path) {
            extract_symbols(&parsed, &mut symbols);
        }
    }

    // Boost focus files
    for sym in &mut symbols {
        if focus.iter().any(|f| sym.file.contains(f)) {
            sym.references += 100;
        }
    }

    // Sort by references (importance)
    symbols.sort_by(|a, b| b.references.cmp(&a.references));

    // Format output within token budget
    let mut output_lines: Vec<String> = Vec::new();
    let mut tokens_used = 0;

    for sym in symbols {
        let line = format!("{} {} ({}:{})", sym.kind, sym.name, sym.file, sym.line);
        let line_tokens = line.len() / 4; // rough estimate

        if tokens_used + line_tokens > max_tokens {
            break;
        }

        output_lines.push(line);
        tokens_used += line_tokens;
    }

    let mut result = Output::ok(format!(
        "Mapped {} files, {} symbols (top {} shown)",
        file_count,
        output_lines.len(),
        output_lines.len()
    ));

    result.data = Some(serde_json::json!({
        "files_scanned": file_count,
        "symbols_shown": output_lines.len(),
        "map": output_lines.join("\n"),
    }));

    Ok(result)
}

fn extract_symbols(parsed: &crate::treesitter::parser::ParsedFile, symbols: &mut Vec<Symbol>) {
    let mut cursor = parsed.tree.walk();

    loop {
        let node = cursor.node();
        let kind = node.kind();

        // Extract function/class/struct definitions
        let symbol_kind = match kind {
            "function_item"
            | "function_definition"
            | "function_declaration"
            | "method_definition" => Some("fn"),
            "struct_item" | "class_definition" | "class_declaration" => Some("struct"),
            "impl_item" => Some("impl"),
            "trait_item" | "interface_declaration" => Some("trait"),
            "enum_item" => Some("enum"),
            "const_item" | "static_item" => Some("const"),
            _ => None,
        };

        if let Some(sk) = symbol_kind {
            // Try to find name child
            if let Some(name_node) = node.child_by_field_name("name") {
                let name = &parsed.source[name_node.byte_range()];
                symbols.push(Symbol {
                    name: name.to_string(),
                    kind: sk.to_string(),
                    file: parsed.path.clone(),
                    line: node.start_position().row + 1,
                    references: 1,
                });
            }
        }

        // Traverse
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
