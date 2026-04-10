// ============================================================================
// CONTEXT DOMAIN -- Token budget math and truncation
// ============================================================================

/// Rough token estimate: 1 token ~= 4 chars.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Truncate content to fit within max_memory_tokens.
/// Prefers sentence boundaries where possible.
pub fn truncate_to_token_budget(content: &str, max_memory_tokens: usize) -> String {
    if estimate_tokens(content) <= max_memory_tokens {
        return content.to_string();
    }
    let max_chars = max_memory_tokens * 4;
    // Try to find a sentence boundary (". ") within the last 40% of the allowed range
    if let Some(cut_point) = content[..max_chars.min(content.len())].rfind(". ") {
        let threshold = (max_chars as f64 * 0.6) as usize;
        if cut_point > threshold {
            return format!("{}. [truncated]", &content[..cut_point]);
        }
    }
    // Hard truncation
    let end = max_chars.min(content.len());
    // Avoid cutting in the middle of a multi-byte char
    let safe_end = if end < content.len() {
        content.floor_char_boundary(end)
    } else {
        end
    };
    format!("{}... [truncated]", &content[..safe_end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens("abcdefghi"), 3);
    }

    #[test]
    fn test_truncate_within_budget() {
        let text = "Hello world.";
        assert_eq!(truncate_to_token_budget(text, 100), text);
    }

    #[test]
    fn test_truncate_sentence_boundary() {
        // Create text that exceeds budget and has a sentence boundary
        let text = "First sentence. Second sentence. Third sentence that goes on for a while to exceed the budget.";
        let result = truncate_to_token_budget(text, 10); // 10 tokens = 40 chars
        assert!(result.ends_with("[truncated]"));
        assert!(result.contains("First sentence"));
    }

    #[test]
    fn test_truncate_hard_cutoff() {
        // Text with no sentence boundary
        let text = "a".repeat(200);
        let result = truncate_to_token_budget(&text, 10); // 10 tokens = 40 chars
        assert!(result.ends_with("... [truncated]"));
        // Content portion should be around 40 chars
        assert!(result.len() < 60);
    }
}
