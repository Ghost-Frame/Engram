// ============================================================================
// ZIP archive parser
// ============================================================================
//
// Ported from the TypeScript `yauzl`-based implementation.
// Iterates ZIP entries, skips junk/hidden/oversized files, detects format
// from file extension, and delegates to the appropriate sub-parser.
// Nested ZIPs are skipped to prevent zip bomb attacks.

use crate::ingestion::types::{ParsedDocument, SupportedFormat};
use crate::validation::{MAX_ZIP_AGGREGATE_SIZE, MAX_ZIP_ENTRY_SIZE};
use crate::Result;
use std::io::{Cursor, Read};

/// Maximum uncompressed size per entry (u64 view of the centralized constant).
const MAX_ZIP_ENTRY_SIZE_U64: u64 = MAX_ZIP_ENTRY_SIZE as u64;
/// Maximum aggregate uncompressed bytes (u64 view of the centralized constant).
const MAX_ZIP_AGGREGATE_SIZE_U64: u64 = MAX_ZIP_AGGREGATE_SIZE as u64;

/// Parse a ZIP archive; returns one ParsedDocument per successfully parsed entry.
pub fn parse(input: &[u8]) -> Result<Vec<ParsedDocument>> {
    let cursor = Cursor::new(input);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| crate::EngError::Internal(format!("ZIP open failed: {}", e)))?;

    let mut documents: Vec<ParsedDocument> = Vec::new();
    let mut aggregate_bytes: u64 = 0;

    for i in 0..archive.len() {
        // Gather metadata in its own scope so the borrow on `archive` ends
        // before we call `by_index` again to read the data.
        let (name, uncompressed_size) = {
            let entry = archive.by_index(i).map_err(|e| {
                crate::EngError::Internal(format!("ZIP entry error at index {}: {}", i, e))
            })?;
            (entry.name().to_string(), entry.size())
        };

        // Skip hidden and system files
        if should_skip(&name) {
            continue;
        }

        let ext = entry_extension(&name);
        let ext_lower = ext.to_lowercase();

        // Skip nested ZIPs to prevent zip bombs
        if ext_lower == ".zip" {
            tracing::debug!("ZIP: skipping nested archive {}", name);
            continue;
        }

        // Size guard against decompression bombs
        if uncompressed_size > MAX_ZIP_ENTRY_SIZE_U64 {
            tracing::warn!(
                "ZIP: skipping {} -- uncompressed size {} bytes exceeds limit",
                name,
                uncompressed_size
            );
            continue;
        }

        if aggregate_bytes.saturating_add(uncompressed_size) > MAX_ZIP_AGGREGATE_SIZE_U64 {
            tracing::warn!(
                "ZIP: skipping {} -- aggregate uncompressed size {} would exceed limit {}",
                name,
                aggregate_bytes.saturating_add(uncompressed_size),
                MAX_ZIP_AGGREGATE_SIZE_U64
            );
            continue;
        }

        // Only process formats we can handle
        let format = match format_from_ext(&ext_lower) {
            Some(f) => f,
            None => continue,
        };

        aggregate_bytes = aggregate_bytes.saturating_add(uncompressed_size);

        // Read the entry bytes
        let data = {
            let mut entry = archive.by_index(i).map_err(|e| {
                crate::EngError::Internal(format!("ZIP read error for {}: {}", name, e))
            })?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| {
                crate::EngError::Internal(format!("Failed to read ZIP entry {}: {}", name, e))
            })?;
            buf
        };

        // Dispatch to the correct parser
        let result = match format {
            SupportedFormat::Pdf => super::pdf::parse(&data),
            SupportedFormat::Docx => super::docx::parse(&data),
            _ => match std::str::from_utf8(&data) {
                Ok(text) => super::parse_with_format(format, text),
                Err(_) => {
                    tracing::warn!("ZIP: {} is not valid UTF-8, skipping", name);
                    continue;
                }
            },
        };

        match result {
            Ok(mut docs) => {
                for doc in &mut docs {
                    doc.source = name.clone();
                    doc.metadata.insert(
                        "zip_entry".to_string(),
                        serde_json::Value::String(name.clone()),
                    );
                }
                documents.extend(docs);
            }
            Err(e) => {
                tracing::warn!("ZIP: error parsing {}: {}", name, e);
            }
        }
    }

    Ok(documents)
}

/// Returns true for entries that should always be ignored.
///
/// Skips:
/// - Hidden files (basename starting with `.`)
/// - macOS `__MACOSX/` artifact directories
/// - Windows junk: `Thumbs.db`, `desktop.ini`
fn should_skip(name: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    if base.starts_with('.') {
        return true;
    }
    if name.contains("__MACOSX/") {
        return true;
    }
    matches!(base, "Thumbs.db" | "desktop.ini")
}

/// Extract the file extension from a ZIP entry name (e.g. `"dir/file.pdf"` -> `".pdf"`).
fn entry_extension(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(pos) => base[pos..].to_string(),
        None => String::new(),
    }
}

/// Map a lowercase file extension to a SupportedFormat.
/// Returns `None` for formats we cannot parse (including `.zip` for nested archives).
fn format_from_ext(ext: &str) -> Option<SupportedFormat> {
    match ext {
        ".md" | ".txt" | ".text" => Some(SupportedFormat::PlainText),
        ".html" | ".htm" => Some(SupportedFormat::Html),
        ".pdf" => Some(SupportedFormat::Pdf),
        ".docx" => Some(SupportedFormat::Docx),
        ".csv" => Some(SupportedFormat::Csv),
        ".jsonl" => Some(SupportedFormat::Jsonl),
        _ => None,
    }
}

/// Detect if input is a ZIP file by extension or magic bytes (PK).
pub fn detect(input: &[u8], extension: Option<&str>) -> bool {
    if let Some(ext) = extension {
        if ext.to_lowercase() == ".zip" {
            return true;
        }
    }
    // ZIP magic bytes: PK (0x50 0x4B 0x03 0x04)
    input.len() >= 4 && input[0] == 0x50 && input[1] == 0x4B && input[2] == 0x03 && input[3] == 0x04
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_skip_hidden() {
        assert!(should_skip(".DS_Store"));
        assert!(should_skip("subdir/.hidden"));
        assert!(!should_skip("readme.md"));
        assert!(!should_skip("dir/file.txt"));
    }

    #[test]
    fn test_should_skip_macosx() {
        assert!(should_skip("__MACOSX/._file.docx"));
        assert!(!should_skip("docs/file.docx"));
    }

    #[test]
    fn test_should_skip_junk() {
        assert!(should_skip("Thumbs.db"));
        assert!(should_skip("desktop.ini"));
        assert!(!should_skip("thumbnail.db"));
    }

    #[test]
    fn test_entry_extension() {
        assert_eq!(entry_extension("dir/file.pdf"), ".pdf");
        assert_eq!(entry_extension("readme.md"), ".md");
        assert_eq!(entry_extension("no_extension"), "");
        assert_eq!(entry_extension("a/b/c.CSV"), ".CSV");
    }

    #[test]
    fn test_format_from_ext() {
        assert_eq!(format_from_ext(".pdf"), Some(SupportedFormat::Pdf));
        assert_eq!(format_from_ext(".docx"), Some(SupportedFormat::Docx));
        assert_eq!(format_from_ext(".md"), Some(SupportedFormat::PlainText));
        assert_eq!(format_from_ext(".zip"), None);
        assert_eq!(format_from_ext(".exe"), None);
    }

    #[test]
    fn test_detect_by_extension() {
        assert!(detect(&[], Some(".zip")));
        assert!(detect(&[], Some(".ZIP")));
        assert!(!detect(&[], Some(".pdf")));
    }

    #[test]
    fn test_detect_by_magic() {
        assert!(detect(&[0x50, 0x4B, 0x03, 0x04, 0x00], None));
        assert!(!detect(&[0x25, 0x50, 0x44, 0x46], None));
    }
}
