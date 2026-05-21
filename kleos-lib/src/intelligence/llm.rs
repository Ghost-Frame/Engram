//! LLM helper -- configurable endpoint for calling a local model (Ollama, etc).
//! Ported from llm/local.ts.

use super::types::LlmOptions;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Shared HTTP client for LLM calls -- avoids per-request TLS/connection-pool
/// setup. 120s timeout matches the old per-call builder.
static LLM_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    // R7-002: hardened builder (connect_timeout + redirect cap) + per-client timeout.
    crate::net::safe_client_builder()
        .timeout(std::time::Duration::from_secs(120))
        .pool_max_idle_per_host(4)
        .build()
        .expect("safe_client_builder failed at LLM client startup")
});

/// Check whether an LLM endpoint is configured.
/// ENGRAM_LLM_URL (legacy) takes precedence; falls back to OLLAMA_URL.
pub fn is_llm_available() -> bool {
    llm_url().is_some()
}

/// Resolve the LLM endpoint URL.
/// ENGRAM_LLM_URL (legacy/undocumented) → OLLAMA_URL (official) → None.
fn llm_url() -> Option<String> {
    std::env::var("ENGRAM_LLM_URL")
        .ok()
        .or_else(|| std::env::var("OLLAMA_URL").ok())
}

/// OpenAI-compatible request body (/v1/chat/completions).
#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    temperature: f64,
    max_tokens: u32,
    stream: bool,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: &'static str,
    content: String,
}

/// OpenAI-compatible response body.
#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Option<Vec<OpenAiChoice>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiChoiceMessage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoiceMessage {
    content: Option<String>,
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
    let url = llm_url().ok_or_else(|| "No LLM URL configured (set OLLAMA_URL)".to_string())?;
    let model = std::env::var("OLLAMA_MODEL")
        .or_else(|_| std::env::var("ENGRAM_LLM_MODEL"))
        .unwrap_or_else(|_| "llama3.2:3b".to_string());
    let api_key = std::env::var("LLM_API_KEY").ok();

    let body = OpenAiRequest {
        model,
        messages: vec![
            OpenAiMessage { role: "system", content: system.to_string() },
            OpenAiMessage { role: "user", content: user.to_string() },
        ],
        temperature: opts.temperature,
        max_tokens: opts.max_tokens,
        stream: false,
    };

    let mut req = LLM_CLIENT.post(&url).json(&body);
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = match resp.text().await {
            Ok(t) => t,
            Err(e) => format!("<failed to read body: {e}>"),
        };
        return Err(format!("LLM returned {}: {}", status, body_text));
    }

    let parsed: OpenAiResponse = resp
        .json()
        .await
        .map_err(|e| format!("LLM response parse error: {}", e))?;

    parsed
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .ok_or_else(|| "LLM response missing content".to_string())
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
