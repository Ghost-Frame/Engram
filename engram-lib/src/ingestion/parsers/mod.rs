// ============================================================================
// Parser registry -- ported from parsers/index.ts
// ============================================================================

pub mod chatgpt;
pub mod claude;
pub mod csv;
pub mod docx;
pub mod html;
pub mod jsonl;
pub mod markdown;
pub mod messages;
pub mod pdf;
pub mod zip;

use super::types::{ParsedDocument, SupportedFormat};
use crate::Result;

/// Parse input text using the specified format.
/// This replaces the TS getParser() registry -- we dispatch via match.
pub fn parse_with_format(format: SupportedFormat, input: &str) -> Result<Vec<ParsedDocument>> {
    match format {
        SupportedFormat::Markdown | SupportedFormat::PlainText => markdown::parse(input),
        SupportedFormat::Html => html::parse(input),
        SupportedFormat::ClaudeExport => claude::parse(input),
        SupportedFormat::ChatGptExport => chatgpt::parse(input),
        SupportedFormat::Messages => messages::parse(input),
        SupportedFormat::Csv => csv::parse(input),
        SupportedFormat::Jsonl => jsonl::parse(input),
        SupportedFormat::Pdf => Err(crate::EngError::Internal(
            "PDF parsing requires binary input; use parse_binary_with_format".to_string(),
        )),
        SupportedFormat::Docx => Err(crate::EngError::Internal(
            "DOCX parsing requires binary input; use parse_binary_with_format".to_string(),
        )),
        SupportedFormat::Zip => Err(crate::EngError::Internal(
            "ZIP parsing requires binary input; use parse_binary_with_format".to_string(),
        )),
    }
}

/// Parse binary input using the specified format (for PDF, DOCX, ZIP).
pub fn parse_binary_with_format(
    format: SupportedFormat,
    input: &[u8],
) -> Result<Vec<ParsedDocument>> {
    match format {
        SupportedFormat::Pdf => pdf::parse(input),
        SupportedFormat::Docx => docx::parse(input),
        SupportedFormat::Zip => zip::parse(input),
        // Text formats: convert to string and delegate
        _ => {
            let text = std::str::from_utf8(input).map_err(|e| {
                crate::EngError::InvalidInput(format!("input is not valid UTF-8: {}", e))
            })?;
            parse_with_format(format, text)
        }
    }
}

/// Check if a format is supported for text parsing.
pub fn is_text_format(format: SupportedFormat) -> bool {
    matches!(
        format,
        SupportedFormat::Markdown
            | SupportedFormat::PlainText
            | SupportedFormat::Html
            | SupportedFormat::ClaudeExport
            | SupportedFormat::ChatGptExport
            | SupportedFormat::Messages
            | SupportedFormat::Csv
            | SupportedFormat::Jsonl
    )
}

/// List all supported format names.
pub fn list_formats() -> Vec<SupportedFormat> {
    vec![
        SupportedFormat::Markdown,
        SupportedFormat::PlainText,
        SupportedFormat::Html,
        SupportedFormat::ClaudeExport,
        SupportedFormat::ChatGptExport,
        SupportedFormat::Messages,
        SupportedFormat::Csv,
        SupportedFormat::Jsonl,
        SupportedFormat::Pdf,
        SupportedFormat::Docx,
        SupportedFormat::Zip,
    ]
}
