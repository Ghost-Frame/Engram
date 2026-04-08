// ============================================================================
// HTML content parser -- ported from parsers/html.ts
// ============================================================================
//
// Implements a simple tag-stripping state machine since we do not have
// an html-to-text crate dependency. Strips script, style, nav, footer,
// header, and aside elements entirely.

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use std::collections::HashMap;

/// Tags whose content should be entirely removed.
const SKIP_TAGS: &[&str] = &["script", "style", "nav", "footer", "header", "aside"];

/// Extract title from <title>...</title> tag.
fn extract_title(html: &str) -> String {
    let lower = html.to_lowercase();
    if let Some(start) = lower.find("<title") {
        if let Some(tag_end) = lower[start..].find('>') {
            let content_start = start + tag_end + 1;
            if let Some(close) = lower[content_start..].find("</title>") {
                let title = &html[content_start..content_start + close];
                let trimmed = title.trim();
                if !trimmed.is_empty() {
                    return trimmed.to_string();
                }
            }
        }
    }
    "Untitled".to_string()
}

/// Strip HTML tags and extract text content.
/// Removes content within SKIP_TAGS entirely.
fn strip_tags(html: &str) -> String {
    let mut output = String::with_capacity(html.len() / 2);
    let lower = html.to_lowercase();
    let bytes = html.as_bytes();
    let lower_bytes = lower.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut skip_depth: Option<String> = None;

    while i < len {
        if bytes[i] == b'<' {
            // Check for comment
            if i + 3 < len && &lower_bytes[i..i + 4] == b"<!--" {
                if let Some(end) = lower[i + 4..].find("-->") {
                    i = i + 4 + end + 3;
                } else {
                    i = len;
                }
                continue;
            }

            // Find end of tag
            let tag_end = match bytes[i..].iter().position(|&b| b == b'>') {
                Some(pos) => i + pos,
                None => {
                    i = len;
                    continue;
                }
            };

            let tag_content = &lower[i + 1..tag_end];
            let is_closing = tag_content.starts_with('/');
            let tag_name_src = if is_closing { &tag_content[1..] } else { tag_content };
            let tag_name = tag_name_src
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");

            // Handle skip tags
            if let Some(ref skipping) = skip_depth {
                if is_closing && tag_name == skipping.as_str() {
                    skip_depth = None;
                }
                i = tag_end + 1;
                continue;
            }

            if !is_closing && SKIP_TAGS.contains(&tag_name) {
                skip_depth = Some(tag_name.to_string());
                i = tag_end + 1;
                continue;
            }

            // Block-level tags get a newline
            if matches!(
                tag_name,
                "p" | "div" | "br" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
                    | "li" | "tr" | "blockquote" | "section" | "article"
            ) {
                output.push('\n');
            }

            i = tag_end + 1;
        } else {
            if skip_depth.is_none() {
                output.push(bytes[i] as char);
            }
            i += 1;
        }
    }

    // Decode common HTML entities
    let output = output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");

    // Collapse excessive whitespace
    let mut result = String::with_capacity(output.len());
    let mut newline_count = 0;
    let mut space_count = 0;

    for ch in output.chars() {
        if ch == '\n' {
            newline_count += 1;
            space_count = 0;
            if newline_count <= 2 {
                result.push('\n');
            }
        } else if ch == ' ' {
            space_count += 1;
            newline_count = 0;
            if space_count <= 1 {
                result.push(' ');
            }
        } else {
            newline_count = 0;
            space_count = 0;
            result.push(ch);
        }
    }

    result
}

/// Parse HTML content into a parsed document.
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    let title = extract_title(input);
    let text = strip_tags(input);

    Ok(vec![ParsedDocument {
        title,
        text,
        metadata: HashMap::new(),
        source: "html".to_string(),
        timestamp: None,
    }])
}

/// Detect if input is HTML by extension or content sniffing.
pub fn detect(input: &str, extension: Option<&str>) -> bool {
    if let Some(ext) = extension {
        let lower = ext.to_lowercase();
        if lower == ".html" || lower == ".htm" {
            return true;
        }
    }
    let trimmed = input.trim_start();
    if trimmed.len() < 5 {
        return false;
    }
    let check = trimmed[..trimmed.len().min(50)].to_lowercase();
    check.starts_with("<!doctype") || check.starts_with("<html")
}
