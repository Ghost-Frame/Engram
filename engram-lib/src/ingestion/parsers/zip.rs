// ============================================================================
// ZIP archive parser -- STUB
// ============================================================================
//
// The TypeScript version uses the `yauzl` crate to extract ZIP entries
// and delegates parsing to sub-parsers per file type.
// TODO: Add `zip` crate dependency to enable ZIP archive parsing.
//
// Logic that would be ported:
// - Skip hidden files, __MACOSX, .DS_Store, Thumbs.db
// - Skip nested ZIPs to prevent zip bombs
// - Size limit per entry (MAX_ZIP_ENTRY_SIZE)
// - Detect format per entry and delegate to appropriate parser

use crate::ingestion::types::ParsedDocument;
use crate::Result;

/// Parse ZIP archive. Currently returns an error as the ZIP parsing
/// dependency is not available.
pub fn parse(_input: &[u8]) -> Result<Vec<ParsedDocument>> {
    Err(crate::EngError::Internal(
        "ZIP archive parsing not available: requires zip crate dependency".to_string(),
    ))
}

/// Detect if input is a ZIP file by extension or magic bytes (PK).
pub fn detect(input: &[u8], extension: Option<&str>) -> bool {
    if let Some(ext) = extension {
        if ext.to_lowercase() == ".zip" {
            return true;
        }
    }
    // ZIP magic bytes: PK (0x50 0x4B 0x03 0x04)
    input.len() >= 4
        && input[0] == 0x50
        && input[1] == 0x4B
        && input[2] == 0x03
        && input[3] == 0x04
}
