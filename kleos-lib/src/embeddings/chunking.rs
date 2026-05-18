use crate::embeddings::EmbeddingProvider;
use crate::Result;
use regex::Regex;
use std::sync::LazyLock;

static SENTENCE_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[.!?]\s").unwrap());

/// Snap `idx` down to the nearest valid UTF-8 char boundary in `text`. Returns
/// `text.len()` when `idx >= text.len()` and `0` for `idx == 0`. Used to keep
/// every byte index handed to `&text[..]` on a codepoint boundary so multi-byte
/// chars (emoji, CJK) at chunk seams cannot trigger a slice panic.
fn floor_char_boundary(text: &str, idx: usize) -> usize {
    if idx >= text.len() {
        return text.len();
    }
    let mut i = idx;
    while i > 0 && !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Chunk `content` and embed each chunk via `embedder`. Returns the
/// (chunk_text, embedding) pairs ready for `StoreRequest::chunk_embeddings`
/// or direct insert into `memory_chunks`.
///
/// Empty or whitespace-only input returns an empty vec. Otherwise the
/// returned vec has at least one entry (a single chunk equal to the full
/// content when length is below `chunk_max_chars`).
pub async fn chunk_and_embed(
    embedder: &dyn EmbeddingProvider,
    content: &str,
    chunk_max_chars: usize,
    chunk_overlap: usize,
    chunk_max_chunks: usize,
) -> Result<Vec<(String, Vec<f32>)>> {
    let chunks = chunk_text_with_limit(content, chunk_max_chars, chunk_overlap, chunk_max_chunks);
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    let embeddings = embedder.embed_batch(&chunks).await?;
    Ok(chunks.into_iter().zip(embeddings).collect())
}

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
        // Floor `end` to a char boundary so the `&text[start..end]` slice
        // below cannot land inside a multi-byte UTF-8 codepoint when
        // chunk_size happens to fall mid-emoji or mid-CJK glyph.
        let end = floor_char_boundary(text, (start + chunk_size).min(text.len()));
        let slice = &text[start..end];

        // Try to break at sentence boundary after 70% of chunk_size.
        // The intra-slice offset is also floored so the inner regex slice is safe.
        let break_search_start = floor_char_boundary(slice, (chunk_size * 7 / 10).min(slice.len()));
        let actual_end = if end < text.len() {
            if let Some(m) = SENTENCE_BREAK.find(&slice[break_search_start..]) {
                let break_pos: usize = break_search_start + m.end();
                start + break_pos
            } else {
                // Fallback: break at last space after 50% of chunk_size.
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
        // Snap the next `start` to a char boundary -- raw byte arithmetic
        // can land mid-codepoint when the previous chunk ended with a
        // multi-byte char, which would panic on the next `&text[start..end]`.
        let raw_next = start + step.max(min_step);
        let next = floor_char_boundary(text, raw_next);
        if next <= start {
            // Floor collapsed back onto start; force at least one codepoint
            // of forward progress so the loop can't stall on dense unicode.
            let mut forced = start + 1;
            while forced < text.len() && !text.is_char_boundary(forced) {
                forced += 1;
            }
            start = forced.min(text.len());
        } else {
            start = next;
        }
    }

    chunks
}

/// Unit tests for chunk_text/chunk_text_with_limit, including regression
/// coverage for the UTF-8 boundary panic fix.
#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a 4-byte emoji straddling the chunk_size byte boundary
    /// used to panic with `start byte index N is not a char boundary`. The
    /// fix snaps every slice endpoint down to a valid char boundary.
    #[test]
    fn emoji_at_chunk_boundary_does_not_panic() {
        // Pad so the 😤 emoji (4 bytes) lands across byte position 100.
        let mut text = "a".repeat(98);
        text.push('😤');
        text.push_str(&"b".repeat(200));
        let chunks = chunk_text(&text, 100, 20);
        assert!(!chunks.is_empty(), "expected at least one chunk");
        for chunk in &chunks {
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
    }

    /// Regression companion: all-emoji input has no ASCII positions at all,
    /// so naive byte slicing would panic on every iteration.
    #[test]
    fn all_emoji_chunks_safely() {
        let text = "🎸".repeat(500); // 500 * 4 bytes = 2000
        let chunks = chunk_text(&text, 200, 40);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
    }

    /// Regression for the May 18 prod panic at `chunking.rs:74`:
    /// after the first iteration the stepped `start` landed inside a
    /// 3-byte em dash, then the next `&text[start..end]` panicked.
    /// Construct a corpus dense with em dashes so wherever the step
    /// lands, it has a high probability of hitting one. With the
    /// pre-fix code this panics; with `floor_char_boundary` applied
    /// to the next `start`, it completes.
    #[test]
    fn em_dash_at_iteration_boundary_does_not_panic() {
        // Pattern: 7 ASCII bytes + 3-byte em dash, repeated. Period = 10
        // bytes, so any step that is not a multiple of 10 has high odds
        // of landing inside an em dash.
        let unit = "abcdefg—";
        let text = unit.repeat(2000); // 20_000 bytes, mixed multi-byte
        let chunks = chunk_text_with_limit(&text, 1440, 160, 6);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
    }

    /// Text shorter than chunk_size returns exactly one chunk equal to the input.
    #[test]
    fn short_text_returns_single_chunk() {
        let text = "Hello world.";
        let chunks = chunk_text(text, 1440, 160);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello world.");
    }

    /// Empty input yields zero chunks.
    #[test]
    fn empty_text_returns_empty() {
        let chunks = chunk_text("", 1440, 160);
        assert!(chunks.is_empty());
    }

    /// Whitespace-only input trims to empty and yields zero chunks.
    #[test]
    fn whitespace_only_returns_empty() {
        let chunks = chunk_text("   \n  ", 1440, 160);
        assert!(chunks.is_empty());
    }

    /// Long text splits into multiple chunks at sentence boundaries below the cap.
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

    /// Successive chunks share `overlap` bytes of context.
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

    /// Chunk count is capped by the explicit `max_chunks` argument regardless of input length.
    #[test]
    fn respects_max_chunks_limit() {
        let text = "Sentence. ".repeat(200);
        let chunks = chunk_text_with_limit(&text, 100, 20, 4);
        assert!(chunks.len() <= 4, "got {} chunks, max was 4", chunks.len());
    }
}
