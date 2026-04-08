//! LLM helper -- configurable endpoint for calling a local model (Ollama, etc).
//! Delegates to LocalModelClient from crate::llm::local.

use tracing::debug;

/// Options for LLM calls.
#[derive(Debug, Clone)]
pub struct LlmOptions {
    pub temperature: f64,
    pub max_tokens: u32,
}

impl Default for LlmOptions {
    fn default() -> Self {
        Self {
            temperature: 0.3,
            max_tokens: 1024,
        }
    }
}

/// Check whether a local LLM endpoint is configured.
pub fn is_llm_available() -> bool {
    // Available if either ENGRAM_LLM_URL is set, or we use the default.
    // Actual connectivity is checked via probe() at runtime.
    true
}

/// Call the local LLM with a system prompt and user content.
/// Returns the raw text response.
pub async fn call_llm(
    system: &str,
    user: &str,
    opts: Option<LlmOptions>,
) -> Result<String, String> {
    let opts = opts.unwrap_or_default();
    let config = crate::llm::local::OllamaConfig::from_env();
    let client = crate::llm::local::LocalModelClient::new(config);

    let call_opts = crate::llm::local::CallOptions {
        temperature: Some(opts.temperature as f32),
        max_tokens: Some(opts.max_tokens),
        priority: crate::llm::local::Priority::Background,
        ..Default::default()
    };

    client
        .call(system, user, Some(call_opts))
        .await
        .map_err(|e| format!("LLM request failed: {}", e))
}

/// Attempt to parse a JSON value from raw LLM output.
/// Strips markdown code fences and repairs common issues.
pub fn repair_and_parse_json<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
    let trimmed = raw.trim();

    // Try direct parse first
    if let Ok(v) = serde_json::from_str::<T>(trimmed) {
        return Some(v);
    }

    // Strip markdown code fences
    let stripped = if trimmed.starts_with("```") {
        let after_first = if let Some(pos) = trimmed.find('\n') {
            &trimmed[pos + 1..]
        } else {
            trimmed.trim_start_matches("```json").trim_start_matches("```")
        };
        after_first.trim_end_matches("```").trim()
    } else {
        trimmed
    };

    if let Ok(v) = serde_json::from_str::<T>(stripped) {
        return Some(v);
    }

    // Try to find JSON object in the text
    if let Some(start) = stripped.find('{') {
        if let Some(end) = stripped.rfind('}') {
            let json_slice = &stripped[start..=end];
            if let Ok(v) = serde_json::from_str::<T>(json_slice) {
                return Some(v);
            }
        }
    }

    debug!("failed to parse JSON from LLM response: {}", &trimmed[..trimmed.len().min(200)]);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct TestJson {
        facts: Vec<String>,
        skip: bool,
    }

    #[test]
    fn test_repair_and_parse_direct() {
        let raw = r#"{"facts": ["a", "b"], "skip": false}"#;
        let result: Option<TestJson> = repair_and_parse_json(raw);
        assert!(result.is_some());
        let r = result.unwrap();
        assert_eq!(r.facts.len(), 2);
        assert!(!r.skip);
    }

    #[test]
    fn test_repair_and_parse_markdown_fenced() {
        let raw = "```json\n{\"facts\": [\"a\"], \"skip\": true}\n```";
        let result: Option<TestJson> = repair_and_parse_json(raw);
        assert!(result.is_some());
        assert!(result.unwrap().skip);
    }

    #[test]
    fn test_repair_and_parse_embedded() {
        let raw = "Here is the result: {\"facts\": [\"x\"], \"skip\": false} done.";
        let result: Option<TestJson> = repair_and_parse_json(raw);
        assert!(result.is_some());
        assert_eq!(result.unwrap().facts, vec!["x"]);
    }

    #[test]
    fn test_repair_and_parse_garbage() {
        let raw = "I can't do that, Dave.";
        let result: Option<TestJson> = repair_and_parse_json(raw);
        assert!(result.is_none());
    }

    #[test]
    fn test_is_llm_available_default() {
        // Always returns true -- actual connectivity checked via probe() at runtime
        assert!(is_llm_available());
    }
}
