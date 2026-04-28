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
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}
