// ============================================================================
// Text chunker -- ported from ingestion/chunker.ts
// ============================================================================

use super::types::{Chunk, ChunkerOptions, ParsedDocument};

const DEFAULT_MAX_CHUNK_SIZE: usize = 3000;
const DEFAULT_OVERLAP: usize = 200;
const DEFAULT_RESPECT_STRUCTURE: bool = true;

/// Find the last occurrence of a heading break (\n followed by 1-6 '#' then whitespace)
/// within window, returning the byte offset or None.
fn find_last_heading_break(window: &str) -> Option<usize> {
    let bytes = window.as_bytes();
    let mut last_pos = None;

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' && i + 1 < bytes.len() && bytes[i + 1] == b'#' {
            let mut hash_count = 0;
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] == b'#' && hash_count < 6 {
                hash_count += 1;
                j += 1;
            }
            if (1..=6).contains(&hash_count) && j < bytes.len() && bytes[j] == b' ' {
                last_pos = Some(i);
            }
        }
        i += 1;
    }

    last_pos
}

/// Find the last occurrence of a sentence break ([.!?] followed by whitespace)
/// within window, returning the byte offset of the char AFTER the punctuation.
fn find_last_sentence_break(window: &str) -> Option<usize> {
    let bytes = window.as_bytes();
    let mut last_pos = None;

    for i in 0..bytes.len().saturating_sub(1) {
        if (bytes[i] == b'.' || bytes[i] == b'!' || bytes[i] == b'?')
            && (bytes[i + 1] == b' ' || bytes[i + 1] == b'\n')
        {
            last_pos = Some(i + 1);
        }
    }

    last_pos
}

/// Try paragraph break, then sentence break as fallback. Returns offset within window.
fn try_paragraph_or_sentence(window: &str, threshold: usize, default_end: usize) -> usize {
    if let Some(para_idx) = window.rfind("\n\n") {
        if para_idx > threshold {
            return para_idx;
        }
    }
    if let Some(sent_idx) = find_last_sentence_break(window) {
        if sent_idx > threshold {
            return sent_idx;
        }
    }
    default_end
}

/// Split a parsed document into chunks with configurable size, overlap,
/// and structure-aware boundary detection.
pub fn chunk_document(doc: &ParsedDocument, options: Option<&ChunkerOptions>) -> Vec<Chunk> {
    let max_size = options
        .and_then(|o| o.max_chunk_size)
        .unwrap_or(DEFAULT_MAX_CHUNK_SIZE);
    let overlap = options.and_then(|o| o.overlap).unwrap_or(DEFAULT_OVERLAP);
    let respect_structure = options
        .and_then(|o| o.respect_structure)
        .unwrap_or(DEFAULT_RESPECT_STRUCTURE);

    let text = doc.text.trim();
    if text.is_empty() {
        return vec![];
    }

    if text.len() <= max_size {
        return vec![Chunk {
            text: text.to_string(),
            index: 0,
            total: 1,
            document_title: doc.title.clone(),
            source: doc.source.clone(),
            metadata: doc.metadata.clone(),
        }];
    }

    let min_advance = max_size * 3 / 10;
    let mut raw_chunks: Vec<String> = Vec::new();
    let mut pos = 0;

    while pos < text.len() {
        let mut end = (pos + max_size).min(text.len());

        if end < text.len() && respect_structure {
            let window = &text[pos..end];
            let threshold_40 = max_size * 4 / 10;
            let threshold_50 = max_size / 2;

            if let Some(h_idx) = find_last_heading_break(window) {
                if h_idx > threshold_40 {
                    end = pos + h_idx;
                } else {
                    end = pos + try_paragraph_or_sentence(window, threshold_50, end - pos);
                }
            } else {
                end = pos + try_paragraph_or_sentence(window, threshold_50, end - pos);
            }
        }

        let chunk = text[pos..end].trim();
        if !chunk.is_empty() {
            raw_chunks.push(chunk.to_string());
        }

        let advance = (end - pos).saturating_sub(overlap);
        pos += advance.max(min_advance);
    }

    let total = raw_chunks.len();
    raw_chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk_text)| Chunk {
            text: chunk_text,
            index,
            total,
            document_title: doc.title.clone(),
            source: doc.source.clone(),
            metadata: doc.metadata.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_doc(text: &str) -> ParsedDocument {
        ParsedDocument {
            title: "Test Doc".to_string(),
            text: text.to_string(),
            metadata: HashMap::new(),
            source: "test".to_string(),
            timestamp: None,
        }
    }

    #[test]
    fn test_empty_text() {
        let doc = make_doc("  ");
        let chunks = chunk_document(&doc, None);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_small_text_no_chunking() {
        let doc = make_doc("Hello world, this is a small text.");
        let chunks = chunk_document(&doc, None);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].total, 1);
        assert_eq!(chunks[0].text, "Hello world, this is a small text.");
    }

    #[test]
    fn test_large_text_multiple_chunks() {
        let sentence = "This is a test sentence with some content. ";
        let text: String = sentence.repeat(200);
        let doc = make_doc(&text);
        let chunks = chunk_document(&doc, None);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert_eq!(chunk.total, chunks.len());
            assert_eq!(chunk.document_title, "Test Doc");
            assert!(!chunk.text.is_empty());
        }
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.index, i);
        }
    }

    #[test]
    fn test_custom_chunk_size() {
        let text = "word ".repeat(1000);
        let doc = make_doc(&text);
        let opts = ChunkerOptions {
            max_chunk_size: Some(500),
            overlap: Some(50),
            respect_structure: Some(false),
        };
        let chunks = chunk_document(&doc, Some(&opts));
        assert!(chunks.len() > 5);
        for chunk in &chunks {
            assert!(chunk.text.len() <= 500);
        }
    }

    #[test]
    fn test_paragraph_boundary_respected() {
        let part1 = "a ".repeat(1400);
        let part2 = "b ".repeat(500);
        let text = format!("{}\n\n{}", part1.trim(), part2.trim());
        let doc = make_doc(&text);
        let chunks = chunk_document(&doc, None);
        assert!(chunks.len() >= 2);
    }
}
