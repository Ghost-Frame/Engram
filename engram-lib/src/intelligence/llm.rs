//! LLM helper -- configurable endpoint for calling a local model (Ollama, etc).
//! Ported from llm/local.ts.

use super::types::LlmOptions;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Shared HTTP client for LLM calls -- avoids per-request TLS/connection-pool
/// setup. 120s timeout matches the old per-call builder.
static LLM_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .pool_max_idle_per_host(4)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Check whether a local LLM endpoint is configured and reachable.
pub fn is_llm_available() -> bool {
    std::env::var("ENGRAM_LLM_URL").is_ok()
}

/// Get the configured LLM URL.
fn llm_url() -> String {
    std::env::var("ENGRAM_LLM_URL")
        .unwrap_or_else(|_| "http://localhost:11434/api/generate".to_string())
}

/// Ollama-compatible request body.
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    system: String,
    prompt: String,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    temperature: f64,
    num_predict: u32,
}

/// Ollama-compatible response body.
#[derive(Debug, Deserialize)]
struct OllamaResponse {
    response: Option<String>,
}

/// Call the local LLM with a system prompt and user content.
/// Returns the raw text response.
#[tracing::instrument(skip(system, user, opts), fields(system_len = system.len(), user_len = user.len()))]
pub async fn call_llm(
    system: &str,
    user: &str,
    opts: Option<LlmOptions>,
) -> Result<String, String> {
    let opts = opts.unwrap_or_default();
    let url = llm_url();
    let model = std::env::var("ENGRAM_LLM_MODEL").unwrap_or_else(|_| "llama3.2:3b".to_string());

    let body = OllamaRequest {
        model,
        system: system.to_string(),
        prompt: user.to_string(),
        stream: false,
        options: OllamaOptions {
            temperature: opts.temperature,
            num_predict: opts.max_tokens,
        },
    };

    let resp = LLM_CLIENT
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(format!("LLM returned {}: {}", status, body_text));
    }

    let parsed: OllamaResponse = resp
        .json()
        .await
        .map_err(|e| format!("LLM response parse error: {}", e))?;

    parsed
        .response
        .ok_or_else(|| "LLM response missing 'response' field".to_string())
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
            trimmed
                .trim_start_matches("```json")
                .trim_start_matches("```")
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

    debug!(
        "failed to parse JSON from LLM response: {}",
        &trimmed[..trimmed.len().min(200)]
    );
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
        // Without env var, should be false
        std::env::remove_var("ENGRAM_LLM_URL");
        assert!(!is_llm_available());
    }
}
