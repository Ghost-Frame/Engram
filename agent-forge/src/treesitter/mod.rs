pub mod parser;

use tree_sitter::Language;

pub fn language_for_extension(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::language()),
        "ts" | "tsx" => Some(tree_sitter_typescript::language_typescript()),
        "js" | "jsx" | "mjs" => Some(tree_sitter_javascript::language()),
        "py" => Some(tree_sitter_python::language()),
        "go" => Some(tree_sitter_go::language()),
        "c" | "h" => Some(tree_sitter_c::language()),
        "json" => Some(tree_sitter_json::language()),
        _ => None,
    }
}

pub fn is_supported_extension(ext: &str) -> bool {
    language_for_extension(ext).is_some()
}
