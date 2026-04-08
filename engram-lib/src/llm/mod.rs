// ============================================================================
// LLM -- Local LLM client, JSON repair utilities.
// Ported from TypeScript llm/index.ts + llm/local.ts
// ============================================================================

pub mod local;

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
            if b <= k { (b, '}') } else { (k, ']') }
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
        if in_string { continue; }
        match bytes[i] {
            b'{' => braces += 1,
            b'}' => braces -= 1,
            b'[' => brackets += 1,
            b']' => brackets -= 1,
            _ => {}
        }
    }
    while brackets > 0 { s.push(']'); brackets -= 1; }
    while braces > 0 { s.push('}'); braces -= 1; }

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
