//! Tree-sitter language registry and extension utilities. Provides the mapping
//! from file extensions to compiled grammars and a convenience predicate for
//! filtering files before attempting to parse them.

pub mod parser;

use tree_sitter::Language;

/// Return the tree-sitter `Language` grammar for a given file extension, or
/// `None` if the extension is not supported by any bundled grammar.
pub fn language_for_extension(ext: &str) -> Option<Language> {
    match ext {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "js" | "jsx" | "mjs" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "go" => Some(tree_sitter_go::LANGUAGE.into()),
        "c" | "h" => Some(tree_sitter_c::LANGUAGE.into()),
        "json" => Some(tree_sitter_json::LANGUAGE.into()),
        _ => None,
    }
}

/// Return `true` if `ext` has a bundled tree-sitter grammar and can be parsed.
pub fn is_supported_extension(ext: &str) -> bool {
    language_for_extension(ext).is_some()
}
