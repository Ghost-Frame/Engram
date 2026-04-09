// ============================================================================
// Format detection -- ported from ingestion/detect.ts
// ============================================================================

use super::types::{FormatMeta, SupportedFormat};

/// Extension to format mapping.
fn ext_to_format(ext: &str) -> Option<SupportedFormat> {
    match ext {
        ".md" => Some(SupportedFormat::Markdown),
        ".txt" | ".text" => Some(SupportedFormat::PlainText),
        ".html" | ".htm" => Some(SupportedFormat::Html),
        ".pdf" => Some(SupportedFormat::Pdf),
        ".docx" => Some(SupportedFormat::Docx),
        ".csv" => Some(SupportedFormat::Csv),
        ".jsonl" => Some(SupportedFormat::Jsonl),
        ".zip" => Some(SupportedFormat::Zip),
        _ => None,
    }
}

/// MIME type to format mapping.
fn mime_to_format(mime: &str) -> Option<SupportedFormat> {
    match mime {
        "text/html" => Some(SupportedFormat::Html),
        "application/pdf" => Some(SupportedFormat::Pdf),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some(SupportedFormat::Docx)
        }
        "text/csv" => Some(SupportedFormat::Csv),
        "application/zip" | "application/x-zip-compressed" => Some(SupportedFormat::Zip),
        "text/plain" => Some(SupportedFormat::PlainText),
        _ => None,
    }
}

/// Check magic bytes for binary format detection.
fn magic_bytes(data: &[u8]) -> Option<SupportedFormat> {
    if data.len() >= 4 {
        // %PDF
        if data[0] == 0x25 && data[1] == 0x50 && data[2] == 0x44 && data[3] == 0x46 {
            return Some(SupportedFormat::Pdf);
        }
        // PK (ZIP archive)
        if data[0] == 0x50 && data[1] == 0x4B && data[2] == 0x03 && data[3] == 0x04 {
            return Some(SupportedFormat::Zip);
        }
    }
    None
}

/// Content-based format sniffing for text inputs.
fn sniff_content(text: &str) -> Option<SupportedFormat> {
    let trimmed = text.trim_start();

    // HTML doctype or opening tag
    if trimmed.len() >= 5 {
        let check_len = trimmed.len().min(50);
        let lower_start = trimmed[..check_len].to_lowercase();
        if lower_start.starts_with("<!doctype") || lower_start.starts_with("<html") {
            return Some(SupportedFormat::Html);
        }
    }

    // JSON array sniffing for chat exports
    if trimmed.starts_with('[') {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(text) {
            if let Some(arr) = parsed.as_array() {
                if let Some(first) = arr.first() {
                    if let Some(obj) = first.as_object() {
                        // Claude export: has "uuid" and "chat_messages"
                        if obj.contains_key("uuid") && obj.contains_key("chat_messages") {
                            return Some(SupportedFormat::ClaudeExport);
                        }
                        // ChatGPT export: has "title" and "mapping"
                        if obj.contains_key("title") && obj.contains_key("mapping") {
                            return Some(SupportedFormat::ChatGptExport);
                        }
                        // Generic messages: has "role" and "content"
                        if obj.contains_key("role") && obj.contains_key("content") {
                            return Some(SupportedFormat::Messages);
                        }
                    }
                }
            }
        }
    }

    None
}

/// Detect the format of input data using extension, MIME type, magic bytes,
/// and content sniffing. Falls back to PlainText.
///
/// Priority order (matching TS):
/// 1. File extension
/// 2. MIME type
/// 3. Magic bytes (binary data only)
/// 4. Content sniffing
/// 5. Fallback to PlainText
pub fn detect_format(input: &[u8], meta: Option<&FormatMeta>) -> SupportedFormat {
    // 1. Extension takes highest priority
    if let Some(m) = meta {
        if let Some(ref ext) = m.extension {
            let ext_lower = ext.to_lowercase();
            if let Some(fmt) = ext_to_format(&ext_lower) {
                return fmt;
            }
        }
    }

    // 2. MIME type
    if let Some(m) = meta {
        if let Some(ref mime) = m.mime {
            let mime_lower = mime.to_lowercase();
            // Strip parameters (e.g. "text/html; charset=utf-8" -> "text/html")
            let base_mime = mime_lower.split(';').next().unwrap_or("").trim();
            if let Some(fmt) = mime_to_format(base_mime) {
                return fmt;
            }
        }
    }

    // 3. Magic bytes
    if let Some(fmt) = magic_bytes(input) {
        return fmt;
    }

    // 4. Content sniffing (interpret as UTF-8 text)
    if let Ok(text) = std::str::from_utf8(input) {
        if let Some(fmt) = sniff_content(text) {
            return fmt;
        }
    }

    // 5. Fallback
    SupportedFormat::PlainText
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extension_detection() {
        let meta = FormatMeta { extension: Some(".md".into()), mime: None };
        assert_eq!(detect_format(b"# Hello", Some(&meta)), SupportedFormat::Markdown);

        let meta = FormatMeta { extension: Some(".csv".into()), mime: None };
        assert_eq!(detect_format(b"a,b,c", Some(&meta)), SupportedFormat::Csv);

        let meta = FormatMeta { extension: Some(".html".into()), mime: None };
        assert_eq!(detect_format(b"hello", Some(&meta)), SupportedFormat::Html);

        let meta = FormatMeta { extension: Some(".txt".into()), mime: None };
        assert_eq!(detect_format(b"hello", Some(&meta)), SupportedFormat::PlainText);
    }

    #[test]
    fn test_mime_detection() {
        let meta = FormatMeta { extension: None, mime: Some("text/html; charset=utf-8".into()) };
        assert_eq!(detect_format(b"stuff", Some(&meta)), SupportedFormat::Html);

        let meta = FormatMeta { extension: None, mime: Some("application/pdf".into()) };
        assert_eq!(detect_format(b"stuff", Some(&meta)), SupportedFormat::Pdf);
    }

    #[test]
    fn test_magic_bytes_pdf() {
        assert_eq!(detect_format(b"%PDF-1.4 fake", None), SupportedFormat::Pdf);
    }

    #[test]
    fn test_magic_bytes_zip() {
        let zip_bytes: &[u8] = &[0x50, 0x4B, 0x03, 0x04, 0x00, 0x00];
        assert_eq!(detect_format(zip_bytes, None), SupportedFormat::Zip);
    }

    #[test]
    fn test_sniff_html() {
        assert_eq!(detect_format(b"<!DOCTYPE html><html></html>", None), SupportedFormat::Html);
        assert_eq!(detect_format(b"<html lang=\"en\"></html>", None), SupportedFormat::Html);
    }

    #[test]
    fn test_sniff_chatgpt_export() {
        let data = br#"[{"title": "Chat", "mapping": {"n": {}}}]"#;
        assert_eq!(detect_format(data, None), SupportedFormat::ChatGptExport);
    }

    #[test]
    fn test_sniff_claude_export() {
        let data = br#"[{"uuid": "abc", "chat_messages": []}]"#;
        assert_eq!(detect_format(data, None), SupportedFormat::ClaudeExport);
    }

    #[test]
    fn test_sniff_messages() {
        let data = br#"[{"role": "user", "content": "hi"}]"#;
        assert_eq!(detect_format(data, None), SupportedFormat::Messages);
    }

    #[test]
    fn test_fallback_plaintext() {
        assert_eq!(detect_format(b"just plain text", None), SupportedFormat::PlainText);
    }

    #[test]
    fn test_extension_priority_over_content() {
        let meta = FormatMeta { extension: Some(".csv".into()), mime: None };
        assert_eq!(detect_format(b"<!DOCTYPE html>", Some(&meta)), SupportedFormat::Csv);
    }
}
