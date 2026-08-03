//! Tree-sitter extraction of bounded semantic units and conservative edges.

use crate::treesitter::parser::ParsedFile;
use tree_sitter::Node;

/// Maximum semantic units retained from one source file.
const MAX_UNITS_PER_FILE: usize = 2_000;
/// Maximum stored source characters for one semantic unit.
const MAX_BODY_CHARS: usize = 16_000;

/// One semantic source unit ready for persistence.
#[derive(Clone, Debug)]
pub(crate) struct ExtractedUnit {
    /// Stable unit kind derived from a grammar node kind.
    pub(crate) kind: String,
    /// Best available short name.
    pub(crate) name: String,
    /// Signature-like first source line.
    pub(crate) signature: String,
    /// Adjacent leading line comments.
    pub(crate) docs: String,
    /// Exact bounded source body.
    pub(crate) body: String,
    /// One-based first line.
    pub(crate) start_line: usize,
    /// One-based final line.
    pub(crate) end_line: usize,
}

/// One syntax-derived relation between a source unit and a target name.
#[derive(Clone, Debug)]
pub(crate) struct ExtractedEdge {
    /// Source unit vector index.
    pub(crate) source_index: usize,
    /// Relation kind.
    pub(crate) kind: String,
    /// Unresolved short target name.
    pub(crate) target_name: String,
    /// One-based first relation line.
    pub(crate) start_line: usize,
    /// One-based final relation line.
    pub(crate) end_line: usize,
    /// Conservative syntactic confidence.
    pub(crate) confidence: f64,
}

/// Extracted representation of one parsed file.
#[derive(Clone, Debug, Default)]
pub(crate) struct ExtractedFile {
    /// Semantic source units.
    pub(crate) units: Vec<ExtractedUnit>,
    /// Relations whose sources refer to unit vector positions.
    pub(crate) edges: Vec<ExtractedEdge>,
}

/// Extract semantic declarations, imports, containment, and call edges.
pub(crate) fn extract_file(parsed: &ParsedFile) -> ExtractedFile {
    let mut output = ExtractedFile::default();
    visit_node(parsed, parsed.tree.root_node(), None, &mut output);
    if output.units.is_empty() {
        output.units.push(unit_from_node(
            parsed,
            parsed.tree.root_node(),
            "file",
            &file_name(&parsed.path),
        ));
    }
    output
}

/// Traverse one syntax node while carrying its nearest enclosing unit.
fn visit_node(
    parsed: &ParsedFile,
    node: Node<'_>,
    parent_unit: Option<usize>,
    output: &mut ExtractedFile,
) {
    if output.units.len() >= MAX_UNITS_PER_FILE {
        return;
    }

    let mut current_unit = parent_unit;
    if let Some(kind) = declaration_kind(node.kind()) {
        let name = node_name(parsed, node)
            .unwrap_or_else(|| format!("{kind}@{}", node.start_position().row + 1));
        let index = output.units.len();
        output.units.push(unit_from_node(parsed, node, kind, &name));
        if let Some(parent_index) = parent_unit {
            output.edges.push(ExtractedEdge {
                source_index: parent_index,
                kind: "contains".to_string(),
                target_name: name.clone(),
                start_line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                confidence: 1.0,
            });
        }
        current_unit = Some(index);
    } else if is_import(node.kind()) {
        let target = normalized_target(node_text(parsed, node));
        if !target.is_empty() {
            let index = output.units.len();
            output
                .units
                .push(unit_from_node(parsed, node, "import", &target));
            output.edges.push(ExtractedEdge {
                source_index: parent_unit.unwrap_or(index),
                kind: "imports".to_string(),
                target_name: target,
                start_line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                confidence: 0.8,
            });
        }
    } else if is_call(node.kind()) {
        if let Some(source_index) = parent_unit {
            let target = call_target(parsed, node);
            if !target.is_empty() {
                output.edges.push(ExtractedEdge {
                    source_index,
                    kind: if is_test_unit(&output.units[source_index]) {
                        "test_of".to_string()
                    } else {
                        "calls".to_string()
                    },
                    target_name: target,
                    start_line: node.start_position().row + 1,
                    end_line: node.end_position().row + 1,
                    confidence: 0.65,
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_node(parsed, child, current_unit, output);
    }
}

/// Map grammar-specific declaration nodes to stable semantic kinds.
fn declaration_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "function_item" | "function_definition" | "function_declaration" => Some("function"),
        "method_definition" | "method_declaration" => Some("method"),
        "struct_item" | "class_definition" | "class_declaration" | "struct_specifier" => {
            Some("class")
        }
        "enum_item" | "enum_declaration" | "enum_specifier" => Some("enum"),
        "trait_item" | "interface_declaration" => Some("interface"),
        "impl_item" => Some("impl"),
        "mod_item" | "module" => Some("module"),
        "type_item" | "type_alias_declaration" | "type_declaration" | "type_definition" => {
            Some("type")
        }
        "const_item" | "static_item" | "const_declaration" => Some("constant"),
        "macro_definition" => Some("macro"),
        "pair" => Some("property"),
        _ => None,
    }
}

/// Return whether a node represents a language import or include declaration.
fn is_import(kind: &str) -> bool {
    matches!(
        kind,
        "use_declaration"
            | "import_statement"
            | "import_from_statement"
            | "import_declaration"
            | "preproc_include"
    )
}

/// Return whether a node represents a function or macro call.
fn is_call(kind: &str) -> bool {
    matches!(kind, "call_expression" | "macro_invocation")
}

/// Build one bounded semantic unit from a syntax node.
fn unit_from_node(parsed: &ParsedFile, node: Node<'_>, kind: &str, name: &str) -> ExtractedUnit {
    let full_body = node_text(parsed, node);
    let signature = full_body
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    ExtractedUnit {
        kind: kind.to_string(),
        name: trim_chars(name.trim(), 300),
        signature: trim_chars(signature, 500),
        docs: leading_docs(&parsed.source, node.start_byte()),
        body: trim_chars(full_body, MAX_BODY_CHARS),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
    }
}

/// Extract the node's named field with grammar-specific fallbacks.
fn node_name(parsed: &ParsedFile, node: Node<'_>) -> Option<String> {
    for field in ["name", "key", "declarator", "type"] {
        if let Some(child) = node.child_by_field_name(field) {
            let value = normalized_target(node_text(parsed, child));
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    let mut cursor = node.walk();
    let fallback = node
        .named_children(&mut cursor)
        .find(|child| {
            matches!(
                child.kind(),
                "identifier" | "type_identifier" | "property_identifier" | "string"
            )
        })
        .map(|child| normalized_target(node_text(parsed, child)))
        .filter(|value| !value.is_empty());
    fallback
}

/// Extract a call's callee text and reduce it to a useful target name.
fn call_target(parsed: &ParsedFile, node: Node<'_>) -> String {
    node.child_by_field_name("function")
        .or_else(|| node.child_by_field_name("macro"))
        .map(|child| normalized_target(node_text(parsed, child)))
        .unwrap_or_default()
}

/// Return a safe source slice for one tree-sitter node.
fn node_text<'source>(parsed: &'source ParsedFile, node: Node<'_>) -> &'source str {
    parsed.source.get(node.byte_range()).unwrap_or("")
}

/// Reduce syntax punctuation and qualification to a short comparable target.
fn normalized_target(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character: char| matches!(character, '"' | '\'' | '`'))
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .filter(|part| !part.is_empty())
        .next_back()
        .unwrap_or("")
        .to_string()
}

/// Collect adjacent leading line comments without crossing a blank line.
fn leading_docs(source: &str, start_byte: usize) -> String {
    let before = source.get(..start_byte).unwrap_or("");
    let mut comments = Vec::new();
    for line in before.lines().rev().take(8) {
        let trimmed = line.trim();
        if trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
        {
            comments.push(trimmed.to_string());
        } else if !trimmed.is_empty() {
            break;
        } else if !comments.is_empty() {
            break;
        }
    }
    comments.reverse();
    comments.join("\n")
}

/// Return whether a unit looks like a conventional test declaration.
fn is_test_unit(unit: &ExtractedUnit) -> bool {
    let name = unit.name.to_ascii_lowercase();
    name.starts_with("test_")
        || name.ends_with("_test")
        || unit.docs.contains("#[test]")
        || unit.body.starts_with("#[test]")
        || unit.body.starts_with("@pytest")
}

/// Return the last path component for fallback file units.
fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

/// Truncate text by Unicode scalar count without breaking UTF-8 boundaries.
fn trim_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
/// Cross-language semantic extraction tests.
mod tests {
    use super::*;
    use crate::treesitter::parser::parse_file;
    use tempfile::tempdir;

    /// Parse a Rust sample and extract declarations plus a call relation.
    #[test]
    fn extracts_rust_units_and_calls() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("sample.rs");
        std::fs::write(
            &path,
            "fn helper() {}\n#[test]\nfn test_helper() { helper(); }\n",
        )
        .unwrap();
        let parsed = parse_file(&path).unwrap();
        let extracted = extract_file(&parsed);
        assert!(extracted.units.iter().any(|unit| unit.name == "helper"));
        assert!(extracted
            .edges
            .iter()
            .any(|edge| edge.target_name == "helper"));
        assert!(extracted.edges.iter().any(|edge| edge.kind == "test_of"));
    }
}
