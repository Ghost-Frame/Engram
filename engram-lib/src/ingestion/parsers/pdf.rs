// ============================================================================
// PDF document parser
// ============================================================================
//
// Uses the `pdf-extract` crate to extract text from PDF files.
// Handles multi-page documents; returns a single ParsedDocument with all text.

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use std::collections::HashMap;

const MAX_PDF_INPUT_BYTES: usize = 100 * 1024 * 1024;
const MAX_PDF_TEXT_BYTES: usize = 100 * 1024 * 1024;

/// Parse PDF binary input into a parsed document.
pub fn parse(input: &[u8]) -> Result<Vec<ParsedDocument>> {
    if input.len() > MAX_PDF_INPUT_BYTES {
        return Err(crate::EngError::InvalidInput(format!(
            "PDF input too large: {} bytes (max {})",
            input.len(),
            MAX_PDF_INPUT_BYTES
        )));
    }

    let text = pdf_extract::extract_text_from_mem(input)
        .map_err(|e| crate::EngError::Internal(format!("PDF extraction failed: {}", e)))?;

    if text.len() > MAX_PDF_TEXT_BYTES {
        return Err(crate::EngError::InvalidInput(format!(
            "PDF extracted text too large: {} bytes (max {})",
            text.len(),
            MAX_PDF_TEXT_BYTES
        )));
    }

    // First non-empty line as title, capped at 100 chars
    let title = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().chars().take(100).collect::<String>())
        .unwrap_or_else(|| "Untitled PDF".to_string());

    let mut metadata = HashMap::new();
    metadata.insert(
        "format".to_string(),
        serde_json::Value::String("pdf".to_string()),
    );

    Ok(vec![ParsedDocument {
        title,
        text,
        metadata,
        source: "pdf".to_string(),
        timestamp: None,
    }])
}

/// Detect if input is a PDF file by extension or magic bytes (%PDF).
pub fn detect(input: &[u8], extension: Option<&str>) -> bool {
    if let Some(ext) = extension {
        if ext.to_lowercase() == ".pdf" {
            return true;
        }
    }
    // %PDF magic bytes: 0x25 0x50 0x44 0x46
    input.len() >= 4 && input[0] == 0x25 && input[1] == 0x50 && input[2] == 0x44 && input[3] == 0x46
}
