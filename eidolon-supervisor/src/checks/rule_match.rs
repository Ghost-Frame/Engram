use super::{CheckType, Rule, Violation};
use regex::Regex;

pub fn check(entry: &serde_json::Value, rules: &[Rule]) -> Vec<Violation> {
    let mut violations = Vec::new();

    let text = extract_check_text(entry);
    if text.is_empty() {
        return violations;
    }

    for rule in rules {
        if !matches!(rule.check_type, CheckType::RuleMatch) {
            continue;
        }

        if let Ok(re) = Regex::new(&rule.pattern) {
            if re.is_match(&text) {
                violations.push(Violation {
                    rule_id: rule.id.clone(),
                    severity: rule.severity.clone(),
                    message: rule.message.clone(),
                    context: truncate(&text, 200),
                    session_id: None,
                });
            }
        }
    }

    violations
}

fn extract_check_text(entry: &serde_json::Value) -> String {
    let obj = match entry.as_object() {
        Some(o) => o,
        None => return String::new(),
    };

    let mut parts = Vec::new();

    // Check tool_input.command (Bash commands)
    if let Some(input) = obj.get("tool_input").or(obj.get("input")) {
        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
            parts.push(cmd.to_string());
        }
        if let Some(content) = input.get("content").and_then(|v| v.as_str()) {
            parts.push(content.to_string());
        }
    }

    // Check assistant text output
    if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
        parts.push(text.to_string());
    }
    if let Some(content) = obj.get("content").and_then(|v| v.as_str()) {
        parts.push(content.to_string());
    }

    // Check commit messages in git operations
    if let Some(msg) = obj.get("message").and_then(|v| v.as_str()) {
        parts.push(msg.to_string());
    }

    parts.join("\n")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        // Take whole chars, not a byte slice: untrusted JSONL may contain
        // multibyte text and `&s[..max]` panics on a non-char-boundary index.
        let truncated: String = s.chars().take(max).collect();
        format!("{}...", truncated)
    }
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate;

    // Multibyte input must never panic on a non-char-boundary byte index and
    // must truncate by whole chars.
    #[test]
    fn truncate_multibyte_does_not_panic() {
        let s = "\u{65e5}\u{672c}\u{8a9e}\u{30c6}\u{30b9}\u{30c8}"; // 6 CJK/kana chars
        let out = truncate(s, 2);
        assert_eq!(out, "\u{65e5}\u{672c}...");
    }

    #[test]
    fn truncate_short_ascii_unchanged() {
        assert_eq!(truncate("hello", 16), "hello");
    }
}
