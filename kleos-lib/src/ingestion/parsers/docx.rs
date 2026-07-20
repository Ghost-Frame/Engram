// ============================================================================
// DOCX document parser
// ============================================================================
//
// DOCX is a ZIP archive of XML files. This parser opens the archive, reads
// `word/document.xml`, and extracts paragraph text from `<w:t>` elements.
// No external XML crate needed -- the OOXML structure is well-defined enough
// for targeted string matching.

use crate::ingestion::types::ParsedDocument;
use crate::validation::MAX_DOCX_XML_BYTES;
use crate::Result;
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// u64 view of the centralized byte limit.
const MAX_DOCX_XML_BYTES_U64: u64 = MAX_DOCX_XML_BYTES as u64;

/// Parse DOCX binary input into a parsed document.
pub fn parse(input: &[u8]) -> Result<Vec<ParsedDocument>> {
    let cursor = Cursor::new(input);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| crate::EngError::Internal(format!("DOCX open failed: {}", e)))?;

    let xml_content = {
        let entry = archive.by_name("word/document.xml").map_err(|_| {
            crate::EngError::Internal("Not a valid DOCX: missing word/document.xml".to_string())
        })?;
        if entry.size() > MAX_DOCX_XML_BYTES_U64 {
            return Err(crate::EngError::InvalidInput(format!(
                "DOCX document.xml too large: {} bytes (max {})",
                entry.size(),
                MAX_DOCX_XML_BYTES_U64
            )));
        }
        let mut content = String::new();
        entry
            .take(MAX_DOCX_XML_BYTES_U64 + 1)
            .read_to_string(&mut content)
            .map_err(|e| {
                crate::EngError::Internal(format!("Failed to read document.xml: {}", e))
            })?;
        if content.len() as u64 > MAX_DOCX_XML_BYTES_U64 {
            return Err(crate::EngError::InvalidInput(format!(
                "DOCX document.xml exceeds max size {}",
                MAX_DOCX_XML_BYTES_U64
            )));
        }
        content
    };

    let text = extract_paragraphs(&xml_content);

    let title = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().chars().take(100).collect::<String>())
        .unwrap_or_else(|| "Untitled Document".to_string());

    let mut metadata = HashMap::new();
    metadata.insert(
        "format".to_string(),
        serde_json::Value::String("docx".to_string()),
    );

    Ok(vec![ParsedDocument {
        title,
        text,
        metadata,
        source: "docx".to_string(),
        timestamp: None,
    }])
}

/// Extract paragraph text from `word/document.xml` content.
///
/// OOXML paragraph structure:
///   <w:p>               -- paragraph boundary
///     <w:r>             -- run (may repeat within paragraph)
///       <w:t ...>text</w:t>
///     </w:r>
///   </w:p>
///
/// We split on `<w:p` to gather paragraphs, then harvest `<w:t>` runs
/// within each and join them. XML entities are decoded.
fn extract_paragraphs(xml: &str) -> String {
    let mut paragraphs: Vec<String> = Vec::new();

    // Byte offsets of every REAL <w:p> paragraph start. A boundary check is
    // required because "<w:p" is also the prefix of <w:pPr>, <w:pStyle>,
    // <w:pgMar> and friends -- the old prefix split minted a phantom
    // paragraph for every paragraph-property tag.
    let mut starts: Vec<usize> = Vec::new();
    let mut from = 0;
    while let Some(pos) = find_tag(&xml[from..], "w:p") {
        starts.push(from + pos);
        from += pos + 4; // advance past "<w:p"
    }

    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(xml.len());
        let mut para_text = String::new();
        let mut remaining = &xml[start..end];

        // Collect all <w:t>...</w:t> runs in this paragraph. find_tag
        // rejects the <w:tbl>/<w:tc>/<w:tr>/<w:tab> prefix collisions the
        // bare substring search fell into.
        while let Some(tag_start) = find_tag(remaining, "w:t") {
            let after_tag_start = &remaining[tag_start..];
            let tag_close = match after_tag_start.find('>') {
                Some(pos) => pos,
                None => break,
            };
            // Self-closing <w:t/> is an empty run: contribute nothing and
            // keep scanning after it, instead of harvesting up to the NEXT
            // run's closing tag.
            if after_tag_start[..tag_close].ends_with('/') {
                remaining = &remaining[tag_start + tag_close + 1..];
                continue;
            }
            let content_start = tag_start + tag_close + 1;
            // Find the closing </w:t>
            let after_content = &remaining[content_start..];
            let close_tag = match after_content.find("</w:t>") {
                Some(pos) => pos,
                None => break,
            };
            let run_text = &remaining[content_start..content_start + close_tag];
            para_text.push_str(&decode_xml_entities(run_text));
            // Advance past </w:t>
            remaining = &remaining[content_start + close_tag + 6..];
        }

        let trimmed = para_text.trim();
        if !trimmed.is_empty() {
            paragraphs.push(trimmed.to_string());
        }
    }

    paragraphs.join("\n\n")
}

/// Find the next occurrence of `<{name}` that is a real tag start: the
/// prefix must be followed by `>`, whitespace (attributes), or `/`
/// (self-closing). Returns the byte offset of the `<`, or None.
fn find_tag(haystack: &str, name: &str) -> Option<usize> {
    let needle = format!("<{name}");
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(&needle) {
        let abs = from + pos;
        match haystack.as_bytes().get(abs + needle.len()) {
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'/') => {
                return Some(abs)
            }
            _ => from = abs + needle.len(),
        }
    }
    None
}

/// Decode the five predefined XML entities.
fn decode_xml_entities(s: &str) -> String {
    // Order matters: &amp; must be last to avoid double-decoding
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Detect if input is a DOCX file by extension or magic bytes (PK zip header).
pub fn detect(input: &[u8], extension: Option<&str>) -> bool {
    if let Some(ext) = extension {
        if ext.to_lowercase() == ".docx" {
            return true;
        }
    }
    // DOCX files are ZIP archives; ZIP magic bytes: 0x50 0x4B (PK)
    input.len() >= 2 && input[0] == 0x50 && input[1] == 0x4B
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_xml_entities() {
        assert_eq!(decode_xml_entities("a &amp; b"), "a & b");
        assert_eq!(decode_xml_entities("&lt;tag&gt;"), "<tag>");
        assert_eq!(decode_xml_entities("&quot;hi&quot;"), "\"hi\"");
        assert_eq!(decode_xml_entities("&apos;s"), "'s");
        // &amp; decoded last avoids double-decode of e.g. &amp;lt;
        assert_eq!(decode_xml_entities("&amp;lt;"), "&lt;");
    }

    #[test]
    fn test_extract_paragraphs_table_cells() {
        // <w:tbl>/<w:tr>/<w:tc> share the "<w:t" prefix; only the real
        // <w:t> runs may contribute text.
        let xml = "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>cell one</w:t></w:r></w:p></w:tc>\
                   <w:tc><w:p><w:r><w:t>cell two</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
        assert_eq!(extract_paragraphs(xml), "cell one\n\ncell two");
    }

    #[test]
    fn test_extract_paragraphs_ppr_no_phantom() {
        // <w:pPr>/<w:pStyle> share the "<w:p" prefix; they must not mint
        // phantom paragraphs (which duplicated run text before the fix).
        let xml = "<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>\
                   <w:r><w:t>only once</w:t></w:r></w:p>";
        assert_eq!(extract_paragraphs(xml), "only once");
    }

    #[test]
    fn test_extract_paragraphs_self_closing_run() {
        // A self-closing <w:t/> is an empty run; text in later runs must
        // survive it un-garbled.
        let xml = "<w:p><w:r><w:t/></w:r><w:r><w:t>after empty</w:t></w:r></w:p>";
        assert_eq!(extract_paragraphs(xml), "after empty");
    }

    #[test]
    fn test_extract_paragraphs_tab_run() {
        // <w:tab/> also shares the "<w:t" prefix inside a run.
        let xml = "<w:p><w:r><w:t>left</w:t></w:r><w:r><w:tab/></w:r>\
                   <w:r><w:t>right</w:t></w:r></w:p>";
        assert_eq!(extract_paragraphs(xml), "leftright");
    }

    #[test]
    fn test_extract_paragraphs_preserve_space_attr() {
        // Attribute form <w:t xml:space="preserve"> still parses.
        let xml = "<w:p><w:r><w:t xml:space=\"preserve\"> spaced </w:t></w:r></w:p>";
        assert_eq!(extract_paragraphs(xml), "spaced");
    }

    #[test]
    fn test_extract_paragraphs_basic() {
        let xml = r#"<w:body>
            <w:p><w:r><w:t>Hello world</w:t></w:r></w:p>
            <w:p><w:r><w:t>Second paragraph</w:t></w:r></w:p>
        </w:body>"#;
        let result = extract_paragraphs(xml);
        assert!(result.contains("Hello world"));
        assert!(result.contains("Second paragraph"));
    }

    #[test]
    fn test_extract_paragraphs_multiple_runs() {
        let xml = r#"<w:p>
            <w:r><w:t>Hello</w:t></w:r>
            <w:r><w:t xml:space="preserve"> world</w:t></w:r>
        </w:p>"#;
        let result = extract_paragraphs(xml);
        assert_eq!(result.trim(), "Hello world");
    }

    #[test]
    fn test_detect_by_extension() {
        assert!(detect(&[], Some(".docx")));
        assert!(detect(&[], Some(".DOCX")));
        assert!(!detect(&[], Some(".pdf")));
    }

    #[test]
    fn test_detect_by_magic() {
        // PK magic bytes
        assert!(detect(&[0x50, 0x4B, 0x03, 0x04], None));
        assert!(!detect(&[0x25, 0x50, 0x44, 0x46], None));
    }
}
