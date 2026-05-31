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
///
/// All offsets are located case-insensitively in `html` itself (tag markers
/// are ASCII), so the original-case title is sliced without building a
/// lowercased shadow whose byte offsets can drift from `html`.
fn extract_title(html: &str) -> String {
    use crate::validation::find_ascii_case_insensitive;
    if let Some(start) = find_ascii_case_insensitive(html, "<title") {
        if let Some(rel_gt) = html[start..].find('>') {
            let content_start = start + rel_gt + 1;
            if let Some(rel_close) = find_ascii_case_insensitive(&html[content_start..], "</title>")
            {
                let title = html[content_start..content_start + rel_close].trim();
                if !title.is_empty() {
                    return title.to_string();
                }
            }
        }
    }
    "Untitled".to_string()
}

/// Strip HTML tags and extract text content.
/// Removes content within SKIP_TAGS entirely.
pub fn strip_tags(html: &str) -> String {
    let mut output = String::with_capacity(html.len() / 2);
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut skip_depth: Option<String> = None;

    while i < len {
        if bytes[i] == b'<' {
            // Check for comment. `<!--` and `-->` are ASCII literals, so byte
            // comparison against `html` is exact and never splits a character.
            if i + 3 < len && &bytes[i..i + 4] == b"<!--" {
                if let Some(end) = html[i + 4..].find("-->") {
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

            // Lowercase only the small tag slice for name comparison. Indexing
            // the original `html` (not a lowercased shadow) keeps byte offsets
            // valid even when a preceding character changes byte length under
            // `to_lowercase`.
            let tag_content = html[i + 1..tag_end].to_lowercase();
            let is_closing = tag_content.starts_with('/');
            let tag_name_src = if is_closing {
                tag_content.strip_prefix('/').unwrap_or(&tag_content)
            } else {
                tag_content.as_str()
            };
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
                "p" | "div"
                    | "br"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "li"
                    | "tr"
                    | "blockquote"
                    | "section"
                    | "article"
            ) {
                output.push('\n');
            }

            i = tag_end + 1;
        } else {
            if skip_depth.is_none() {
                // Decode full UTF-8 char (bytes[i] as char is mojibake for multi-byte)
                let ch = html[i..].chars().next().unwrap();
                output.push(ch);
            }
            i += html[i..].chars().next().map_or(1, |c| c.len_utf8());
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
    let check = crate::validation::truncate_on_char_boundary(trimmed, 50).to_lowercase();
    check.starts_with("<!doctype") || check.starts_with("<html")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a character before the title whose lowercase has a different
    /// byte length (here 'İ' -> "i̇") used to desync lowercased offsets from the
    /// original slice and return a garbled or mid-character title.
    #[test]
    fn extract_title_handles_multibyte_prefix() {
        let html = "İ<title>Café Menu</title><body>x</body>";
        assert_eq!(extract_title(html), "Café Menu");
    }

    /// Title tag matching is case-insensitive and absent titles fall back.
    #[test]
    fn extract_title_case_insensitive_and_fallback() {
        assert_eq!(extract_title("<HTML><TITLE>Hello</TITLE>"), "Hello");
        assert_eq!(extract_title("<p>no title here</p>"), "Untitled");
    }

    /// Multibyte text content and a length-changing prefix character must not
    /// panic the tag-stripping state machine, and skipped tags stay excluded.
    #[test]
    fn strip_tags_multibyte_content_no_panic() {
        let html = "İ<p>café 🎉 日本語</p><script>secretpayload</script>tail";
        let text = strip_tags(html);
        assert!(text.contains("café 🎉 日本語"), "got: {text:?}");
        assert!(!text.contains("secretpayload"));
        assert!(text.contains("tail"));
    }
}
