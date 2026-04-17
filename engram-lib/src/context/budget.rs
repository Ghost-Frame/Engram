// ============================================================================
// CONTEXT DOMAIN -- Token budget math and truncation
// ============================================================================

use super::types::MAX_TOKEN_BUDGET;

/// Rough token estimate: 1 token ~= 4 chars.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

// -- Model-aware context budgeting (3.2) -------------------------------------

/// Known model context window sizes (in tokens).
/// When a caller passes `model_id`, we auto-budget to 80% of the window.
#[derive(Debug, Clone, Copy)]
pub struct ModelProfile {
    pub context_window: usize,
}

/// Look up a model's context window by name.
/// Recognises common prefixes/patterns so callers don't need exact IDs.
pub fn resolve_model_profile(model_id: &str) -> Option<ModelProfile> {
    let m = model_id.to_lowercase();

    // Claude models
    if m.starts_with("claude-3.5") || m.starts_with("claude-3-5") {
        return Some(ModelProfile {
            context_window: 200_000,
        });
    }
    if m.starts_with("claude-3-opus") || m.starts_with("claude-3.0-opus") {
        return Some(ModelProfile {
            context_window: 200_000,
        });
    }
    if m.starts_with("claude-3") {
        // Sonnet, Haiku, etc.
        return Some(ModelProfile {
            context_window: 200_000,
        });
    }
    if m.starts_with("claude-4") {
        return Some(ModelProfile {
            context_window: 200_000,
        });
    }
    if m.starts_with("claude-2") {
        return Some(ModelProfile {
            context_window: 100_000,
        });
    }

    // OpenAI GPT-4o / GPT-4-turbo family
    if m.starts_with("gpt-4o") {
        return Some(ModelProfile {
            context_window: 128_000,
        });
    }
    if m.starts_with("gpt-4-turbo") || m.starts_with("gpt-4-1106") || m.starts_with("gpt-4-0125") {
        return Some(ModelProfile {
            context_window: 128_000,
        });
    }
    if m.starts_with("gpt-4") {
        return Some(ModelProfile {
            context_window: 8_192,
        });
    }
    if m.starts_with("gpt-3.5-turbo-16k") {
        return Some(ModelProfile {
            context_window: 16_384,
        });
    }
    if m.starts_with("gpt-3.5") || m.starts_with("gpt-35") {
        return Some(ModelProfile {
            context_window: 4_096,
        });
    }
    if m.starts_with("o1") || m.starts_with("o3") || m.starts_with("o4") {
        return Some(ModelProfile {
            context_window: 200_000,
        });
    }

    // Google Gemini
    if m.starts_with("gemini-2") || m.starts_with("gemini-1.5-pro") {
        return Some(ModelProfile {
            context_window: 1_000_000,
        });
    }
    if m.starts_with("gemini-1.5-flash") {
        return Some(ModelProfile {
            context_window: 1_000_000,
        });
    }
    if m.starts_with("gemini-1.0") || m.starts_with("gemini-pro") {
        return Some(ModelProfile {
            context_window: 32_000,
        });
    }

    // Mistral / Mixtral
    if m.contains("mixtral") {
        return Some(ModelProfile {
            context_window: 32_768,
        });
    }
    if m.contains("mistral-large") || m.contains("mistral-medium") {
        return Some(ModelProfile {
            context_window: 128_000,
        });
    }
    if m.contains("mistral") {
        return Some(ModelProfile {
            context_window: 32_768,
        });
    }

    // Llama 3.x
    if m.contains("llama-3") || m.contains("llama3") {
        return Some(ModelProfile {
            context_window: 128_000,
        });
    }
    if m.contains("llama-2") || m.contains("llama2") {
        return Some(ModelProfile {
            context_window: 4_096,
        });
    }

    // DeepSeek
    if m.contains("deepseek") {
        return Some(ModelProfile {
            context_window: 128_000,
        });
    }

    // Cohere Command R
    if m.contains("command-r") {
        return Some(ModelProfile {
            context_window: 128_000,
        });
    }

    None
}

/// Default budget utilization ratio: 80% of the model's context window.
const MODEL_BUDGET_RATIO: f64 = 0.80;

/// Given an optional explicit budget and an optional model_id, resolve the
/// effective token budget. Model-based budgets are capped at MAX_TOKEN_BUDGET.
///
/// Priority: explicit budget > model-derived budget > DEFAULT_TOKEN_BUDGET.
pub fn resolve_budget(
    explicit: Option<usize>,
    model_id: Option<&str>,
    default: usize,
) -> (usize, Option<&'static str>) {
    if let Some(budget) = explicit {
        return (budget.min(MAX_TOKEN_BUDGET), None);
    }
    if let Some(mid) = model_id {
        if let Some(profile) = resolve_model_profile(mid) {
            let derived = (profile.context_window as f64 * MODEL_BUDGET_RATIO) as usize;
            let capped = derived.min(MAX_TOKEN_BUDGET);
            let note = if derived > MAX_TOKEN_BUDGET {
                Some("budget capped at MAX_TOKEN_BUDGET")
            } else {
                None
            };
            return (capped, note);
        }
    }
    (default.min(MAX_TOKEN_BUDGET), None)
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
        // MSRV 1.85: floor_char_boundary is stable since 1.91, so walk back
        // from `end` to the nearest UTF-8 char boundary manually.
        let mut i = end;
        while i > 0 && !content.is_char_boundary(i) {
            i -= 1;
        }
        i
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

    // -- Model-aware budgeting tests --

    #[test]
    fn test_resolve_model_profile_known() {
        let p = resolve_model_profile("claude-3.5-sonnet").unwrap();
        assert_eq!(p.context_window, 200_000);

        let p = resolve_model_profile("gpt-4o-2024-05-13").unwrap();
        assert_eq!(p.context_window, 128_000);

        let p = resolve_model_profile("gpt-4").unwrap();
        assert_eq!(p.context_window, 8_192);

        let p = resolve_model_profile("gemini-1.5-pro").unwrap();
        assert_eq!(p.context_window, 1_000_000);

        let p = resolve_model_profile("llama-3.1-70b").unwrap();
        assert_eq!(p.context_window, 128_000);
    }

    #[test]
    fn test_resolve_model_profile_unknown() {
        assert!(resolve_model_profile("my-custom-model-v9").is_none());
    }

    #[test]
    fn test_resolve_budget_explicit_wins() {
        let (budget, _note) = resolve_budget(Some(4000), Some("claude-3.5-sonnet"), 8000);
        assert_eq!(budget, 4000);
    }

    #[test]
    fn test_resolve_budget_model_derived() {
        // gpt-4o: 128k * 0.8 = 102400, under MAX_TOKEN_BUDGET (64k) -> capped
        let (budget, note) = resolve_budget(None, Some("gpt-4o"), 8000);
        assert_eq!(budget, MAX_TOKEN_BUDGET); // capped
        assert!(note.is_some());
    }

    #[test]
    fn test_resolve_budget_small_model() {
        // gpt-3.5-turbo: 4096 * 0.8 = 3276
        let (budget, note) = resolve_budget(None, Some("gpt-3.5-turbo"), 8000);
        assert_eq!(budget, 3276);
        assert!(note.is_none());
    }

    #[test]
    fn test_resolve_budget_unknown_model_falls_to_default() {
        let (budget, _note) = resolve_budget(None, Some("unknown-model"), 8000);
        assert_eq!(budget, 8000);
    }

    #[test]
    fn test_resolve_budget_no_model_no_explicit() {
        let (budget, _note) = resolve_budget(None, None, 8000);
        assert_eq!(budget, 8000);
    }
}
