// ============================================================================
// PDF document parser -- STUB
// ============================================================================
//
// The TypeScript version uses the `pdf-parse` crate for PDF extraction.
// TODO: Add `pdf-extract` or `lopdf` crate dependency to enable PDF parsing.

use crate::ingestion::types::ParsedDocument;
use crate::Result;

/// Parse PDF input. Currently returns an error as the PDF parsing
/// dependency is not available.
pub fn parse(_input: &[u8]) -> Result<Vec<ParsedDocument>> {
    Err(crate::EngError::Internal(
        "PDF parsing not available: requires pdf-extract or lopdf crate dependency".to_string(),
    ))
}

/// Detect if input is a PDF file by extension or magic bytes (%PDF).
pub fn detect(input: &[u8], extension: Option<&str>) -> bool {
    if let Some(ext) = extension {
        if ext.to_lowercase() == ".pdf" {
            return true;
        }
    }
    // %PDF magic bytes: 0x25 0x50 0x44 0x46
    input.len() >= 4
        && input[0] == 0x25
        && input[1] == 0x50
        && input[2] == 0x44
        && input[3] == 0x46
}
