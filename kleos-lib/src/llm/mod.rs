// ============================================================================
// LLM -- Local LLM client, JSON repair utilities.
// Ported from TypeScript llm/index.ts + llm/local.ts
// ============================================================================

pub mod local;
pub mod prompts;
pub mod template;
pub mod types;

pub use types::*;

/// Parse the `KLEOS_LLM_THINK` env value into an optional thinking-mode setting.
///
/// Accepts (case-insensitive, trimmed):
///   - `"1"`, `"true"`, `"yes"`, `"on"`   -> `Some(true)`  (reasoning ON)
///   - `"0"`, `"false"`, `"no"`, `"off"`  -> `Some(false)` (reasoning OFF)
///   - absent or empty                    -> `None`        (no-op, leave it to the model)
///   - any other value                    -> `None` + a warning
///
/// `None` is the neutral default: callers inject nothing, so the request body is
/// byte-identical to one built without this feature and the model keeps its own
/// default thinking behaviour.
fn parse_think_setting(raw: Option<String>) -> Option<bool> {
    let value = raw?;
    match value.trim().to_ascii_lowercase().as_str() {
        "" => None,
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        other => {
            tracing::warn!(
                "unrecognised KLEOS_LLM_THINK={:?}, ignoring (leaving thinking mode to the model)",
                other
            );
            None
        }
    }
}

/// Resolve the thinking-mode setting from the environment.
///
/// Reads `KLEOS_LLM_THINK` (canonical) with a legacy `ENGRAM_LLM_THINK` fallback
/// via [`crate::kleos_env`]. Returns `None` when unset/empty so injection is a
/// no-op. See [`parse_think_setting`] for the accepted values.
pub fn think_setting() -> Option<bool> {
    parse_think_setting(crate::kleos_env("LLM_THINK").ok())
}

/// Inject the `think` and `reasoning_effort` fields into an OpenAI-compat /
/// Ollama request body, driven by [`think_setting`].
///
/// When the operator has not set `KLEOS_LLM_THINK` (or set it empty), this is a
/// no-op and the body is left untouched -- preserving the upstream default where
/// the model decides. When set, it injects `think = <bool>` and the
/// OpenAI-standard `reasoning_effort` (`"high"` when ON, `"none"` when OFF), so
/// the toggle works on both Ollama endpoint styles (Ollama silently ignores the
/// `think` field on `/v1/chat/completions`, honouring `reasoning_effort`
/// instead; the native `/api/*` endpoints honour `think`).
///
/// Idempotent: only inserts keys when absent, so a value already set by the
/// caller is preserved. No-op on non-object JSON values.
pub(crate) fn inject_openai_compat_reasoning(body: &mut serde_json::Value) {
    inject_reasoning_setting(body, think_setting());
}

/// Core injector applying an explicit thinking-mode `setting`.
///
/// `None` is a no-op (body untouched). Lets callers that have already resolved
/// the setting (e.g. to gate an expensive body round-trip on `is_some()`) reuse
/// the injection logic without re-reading the environment. Idempotent: only
/// inserts keys when absent. No-op on non-object values.
pub(crate) fn inject_reasoning_setting(body: &mut serde_json::Value, setting: Option<bool>) {
    let Some(think) = setting else {
        return;
    };
    if let serde_json::Value::Object(ref mut map) = body {
        map.entry("think".to_string())
            .or_insert_with(|| serde_json::Value::Bool(think));
        let effort = if think { "high" } else { "none" };
        map.entry("reasoning_effort".to_string())
            .or_insert_with(|| serde_json::Value::String(effort.to_string()));
    }
}

/// Repair and parse JSON from LLM output that may have common formatting issues.
///
/// Handles: markdown code fences, trailing commas, unterminated strings,
/// unbalanced braces/brackets. Returns None if the input cannot be repaired.
pub fn repair_and_parse_json(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();

    // 1. Find first { or [ and corresponding last } or ]
    let first_brace = trimmed.find('{');
    let first_bracket = trimmed.find('[');

    let (start, close_char) = match (first_brace, first_bracket) {
        (None, None) => return None,
        (Some(b), None) => (b, '}'),
        (None, Some(k)) => (k, ']'),
        (Some(b), Some(k)) => {
            if b <= k {
                (b, '}')
            } else {
                (k, ']')
            }
        }
    };

    let last_close = trimmed.rfind(close_char).unwrap_or(start);
    let mut s = if last_close > start {
        trimmed[start..=last_close].to_string()
    } else {
        trimmed[start..].to_string()
    };

    // 2. Strip markdown fences
    s = s.replace("```json", "").replace("```", "");

    // 3. Fix trailing commas before } or ]
    s = fix_trailing_commas(&s);

    // 4. Try parse
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
        return Some(v);
    }

    // 5. Fix unterminated strings
    let mut in_str = false;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            in_str = !in_str;
        }
    }
    if in_str {
        s.push('"');
    }

    // 6. Balance braces/brackets
    let mut braces: i32 = 0;
    let mut brackets: i32 = 0;
    let mut in_string = false;
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'"' && (i == 0 || bytes[i - 1] != b'\\') {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match bytes[i] {
            b'{' => braces += 1,
            b'}' => braces -= 1,
            b'[' => brackets += 1,
            b']' => brackets -= 1,
            _ => {}
        }
    }
    while brackets > 0 {
        s.push(']');
        brackets -= 1;
    }
    while braces > 0 {
        s.push('}');
        braces -= 1;
    }

    // 7. Fix trailing commas again
    s = fix_trailing_commas(&s);

    // 8. Try parse again
    serde_json::from_str::<serde_json::Value>(&s).ok()
}

/// Remove trailing commas before } or ]
fn fix_trailing_commas(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ',' {
            // Look ahead past whitespace for } or ]
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                // Skip the comma, keep whitespace and closing bracket
                i += 1;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_setting_unset_or_empty_is_none() {
        assert_eq!(parse_think_setting(None), None);
        assert_eq!(parse_think_setting(Some(String::new())), None);
        assert_eq!(parse_think_setting(Some("   ".to_string())), None);
    }

    #[test]
    fn think_setting_truthy_values() {
        for v in ["1", "true", "TRUE", "Yes", "on"] {
            assert_eq!(parse_think_setting(Some(v.to_string())), Some(true), "{v}");
        }
    }

    #[test]
    fn think_setting_falsy_values() {
        // "0" must yield Some(false) so reasoning is explicitly turned off.
        for v in ["0", "false", "No", "OFF"] {
            assert_eq!(parse_think_setting(Some(v.to_string())), Some(false), "{v}");
        }
    }

    #[test]
    fn think_setting_unrecognised_is_none() {
        assert_eq!(parse_think_setting(Some("maybe".to_string())), None);
    }

    #[test]
    fn inject_is_noop_on_non_object() {
        // Non-object bodies are returned untouched regardless of setting.
        let mut v = serde_json::json!("a string");
        inject_reasoning_setting(&mut v, Some(true));
        assert_eq!(v, serde_json::json!("a string"));
    }

    #[test]
    fn inject_reasoning_setting_none_is_noop() {
        // The unset path (None) leaves the body byte-identical -- this is what
        // call_llm_endpoint relies on to skip the serde round-trip entirely.
        let mut v = serde_json::json!({"model": "x"});
        inject_reasoning_setting(&mut v, None);
        assert_eq!(v, serde_json::json!({"model": "x"}));
    }

    #[test]
    fn inject_reasoning_setting_false_injects_off() {
        let mut v = serde_json::json!({"model": "x"});
        inject_reasoning_setting(&mut v, Some(false));
        assert_eq!(v["think"], serde_json::json!(false));
        assert_eq!(v["reasoning_effort"], serde_json::json!("none"));
    }

    #[test]
    fn inject_reasoning_setting_preserves_caller_value() {
        let mut v = serde_json::json!({"model": "x", "think": true});
        inject_reasoning_setting(&mut v, Some(false));
        // Caller-set value wins (idempotent or_insert_with).
        assert_eq!(v["think"], serde_json::json!(true));
    }

    #[test]
    fn test_clean_json() {
        let input = r#"{"key": "value"}"#;
        let v = repair_and_parse_json(input).unwrap();
        assert_eq!(v["key"], "value");
    }

    #[test]
    fn test_markdown_fenced() {
        let v = repair_and_parse_json(r#"prefix {"key": "value"} suffix"#).unwrap();
        assert_eq!(v["key"], "value");
    }

    #[test]
    fn test_markdown_fenced_with_backticks() {
        let input = "prefix ```json {\"a\": 1} ``` suffix";
        let v = repair_and_parse_json(input).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_trailing_comma() {
        let input = r#"{"a": 1, "b": 2, }"#;
        let v = repair_and_parse_json(input).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], 2);
    }

    #[test]
    fn test_no_json() {
        assert!(repair_and_parse_json("no json here").is_none());
    }

    #[test]
    fn test_unbalanced_braces() {
        let input = r#"{"a": {"b": 1}"#;
        let v = repair_and_parse_json(input).unwrap();
        assert_eq!(v["a"]["b"], 1);
    }

    #[test]
    fn test_array_input() {
        let input = r#"[1, 2, 3]"#;
        let v = repair_and_parse_json(input).unwrap();
        assert_eq!(v[0], 1);
        assert_eq!(v[2], 3);
    }

    #[test]
    fn test_prefix_text() {
        let input = r#"Here is the JSON: {"result": true} hope that helps"#;
        let v = repair_and_parse_json(input).unwrap();
        assert_eq!(v["result"], true);
    }
}
