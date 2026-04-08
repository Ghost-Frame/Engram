// ============================================================================
// Markdown document parser -- ported from parsers/markdown.ts
// ============================================================================

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use std::collections::HashMap;

/// Parse markdown text into a single parsed document.
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    // Extract title from first heading
    let title = find_first_heading(input).unwrap_or_else(|| {
        input.chars().take(60).collect::<String>()
    });

    Ok(vec![ParsedDocument {
        title,
        text: input.to_string(),
        metadata: HashMap::new(),
        source: "markdown".to_string(),
        timestamp: None,
    }])
}

/// Find the first markdown heading (# through ######).
fn find_first_heading(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            let hash_count = trimmed.bytes().take_while(|&b| b == b'#').count();
            if (1..=6).contains(&hash_count) {
                let rest = &trimmed[hash_count..];
                if rest.starts_with(' ') || rest.is_empty() {
                    let title = rest.trim().to_string();
                    if !title.is_empty() {
                        return Some(title);
                    }
                }
            }
        }
    }
    None
}

/// Detect if input has a markdown/text extension.
pub fn detect(extension: Option<&str>) -> bool {
    match extension {
        Some(ext) => {
            let lower = ext.to_lowercase();
            lower == ".md" || lower == ".txt" || lower == ".text"
        }
        None => false,
    }
}
