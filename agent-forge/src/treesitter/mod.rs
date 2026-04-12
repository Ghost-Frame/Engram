pub mod parser;

use tree_sitter::Language;

pub fn language_for_extension(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ts" | "tsx" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "js" | "jsx" | "mjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "json" => Some(tree_sitter_json::LANGUAGE.into()),
        _ => None,
    }
}

pub fn is_supported_extension(ext: &str) -> bool {
    language_for_extension(ext).is_some()
}
