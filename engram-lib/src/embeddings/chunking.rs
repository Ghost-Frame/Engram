use regex::Regex;
use std::sync::LazyLock;

static SENTENCE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!?]\s").unwrap());

/// Split text into overlapping chunks of roughly `chunk_size` characters
/// with `overlap` chars of context. Default max 6 chunks.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    chunk_text_with_limit(text, chunk_size, overlap, 6)
}

/// Split text into overlapping chunks with an explicit max_chunks limit.
pub fn chunk_text_with_limit(
    text: &str,
    chunk_size: usize,
    overlap: usize,
    max_chunks: usize,
) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return vec![];
    }
    if text.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() && chunks.len() < max_chunks {
        let end = (start + chunk_size).min(text.len());
        let slice = &text[start..end];

        // Try to break at sentence boundary after 70% of chunk_size
        let break_search_start = chunk_size * 7 / 10;
        let actual_end = if end < text.len() {
            if let Some(m) = SENTENCE_BREAK.find(&slice[break_search_start.min(slice.len())..]) {
                let break_pos: usize = break_search_start.min(slice.len()) + m.end();
                start + break_pos
            } else {
                // Fallback: break at last space after 50% of chunk_size
                let half = chunk_size / 2;
                let search_region = &text[start..end];
                if let Some(pos) = search_region.rfind(' ') {
                    if pos > half {
                        start + pos
                    } else {
                        end
                    }
                } else {
                    end
                }
            }
        } else {
            end
        };

        let chunk = text[start..actual_end].trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }

        let step = (actual_end - start).saturating_sub(overlap);
        let min_step = chunk_size * 3 / 10;
        start += step.max(min_step);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_returns_single_chunk() {
        let text = "Hello world.";
        let chunks = chunk_text(text, 1440, 160);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world.");
    }

    #[test]
    fn empty_text_returns_empty() {
        let chunks = chunk_text("", 1440, 160);
        assert!(chunks.is_empty());
    }

    #[test]
    fn whitespace_only_returns_empty() {
        let chunks = chunk_text("   \n  ", 1440, 160);
        assert!(chunks.is_empty());
    }

    #[test]
    fn long_text_splits_at_sentence_boundary() {
        let text = "First sentence here. Second sentence here. Third sentence here. Fourth sentence here. Fifth sentence here.";
        let chunks = chunk_text(text, 60, 10);
        assert!(
            chunks.len() > 1,
            "expected multiple chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            assert!(chunk.len() <= 70, "chunk too long: {} chars", chunk.len());
        }
    }

    #[test]
    fn chunks_overlap() {
        let text = "Word ".repeat(100);
        let chunks = chunk_text(&text, 100, 30);
        assert!(chunks.len() > 1);
        if chunks.len() >= 2 {
            let end_of_first = &chunks[0][chunks[0].len().saturating_sub(20)..];
            let trimmed = end_of_first.trim();
            assert!(
                chunks[1].contains(trimmed) || trimmed.is_empty(),
                "no overlap detected between chunks"
            );
        }
    }

    #[test]
    fn respects_max_chunks_limit() {
        let text = "Sentence. ".repeat(200);
        let chunks = chunk_text_with_limit(&text, 100, 20, 4);
        assert!(chunks.len() <= 4, "got {} chunks, max was 4", chunks.len());
    }
}
