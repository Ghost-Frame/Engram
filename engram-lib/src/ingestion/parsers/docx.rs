// ============================================================================
// DOCX document parser -- STUB
// ============================================================================
//
// The TypeScript version uses the `mammoth` crate for DOCX parsing.
// TODO: Add `docx-rs` or similar crate dependency to enable DOCX parsing.

use crate::ingestion::types::ParsedDocument;
use crate::Result;

/// Parse DOCX input. Currently returns an error as the DOCX parsing
/// dependency is not available.
pub fn parse(_input: &[u8]) -> Result<Vec<ParsedDocument>> {
    Err(crate::EngError::Internal(
        "DOCX parsing not available: requires docx-rs or similar crate dependency".to_string(),
    ))
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
