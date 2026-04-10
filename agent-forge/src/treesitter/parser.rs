use std::path::Path;
use tree_sitter::{Parser, Tree};

use super::language_for_extension;

pub struct ParsedFile {
    pub path: String,
    pub tree: Tree,
    pub source: String,
}

pub fn parse_file(path: &Path) -> Option<ParsedFile> {
    let ext = path.extension()?.to_str()?;
    let language = language_for_extension(ext)?;

    let source = std::fs::read_to_string(path).ok()?;

    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;

    let tree = parser.parse(&source, None)?;

    Some(ParsedFile {
        path: path.to_string_lossy().to_string(),
        tree,
        source,
    })
}
