// ============================================================================
// LLM -- Template interpolation utilities.
//
// Lightweight `{{placeholder}}` interpolation shared by the LLM call sites
// (Broca, Chiasm, growth reflections, context inference, ...) so any caller
// can render a templated prompt without depending on a particular service
// module. Pairs with `llm::prompts`, whose `load_and_render` helper renders
// an (optionally overridden) prompt through `interpolate`.
// ============================================================================

/// Resolve a dot-separated path inside a JSON value.
///
/// Walks `path` segment by segment starting from `obj`. Returns `Null` as
/// soon as a segment is missing or a non-object is traversed.
pub fn resolve_dot_path(obj: &serde_json::Value, path: &str) -> serde_json::Value {
    let mut current = obj;
    for key in path.split('.') {
        match current {
            serde_json::Value::Object(map) => {
                if let Some(val) = map.get(key) {
                    current = val;
                } else {
                    return serde_json::Value::Null;
                }
            }
            _ => return serde_json::Value::Null,
        }
    }
    current.clone()
}

/// Replace `{{path}}` placeholders in a template string with values from
/// `vars`.
///
/// `path` can be a dot-separated lookup (`agent.name`). Strings are inserted
/// verbatim, `Null` becomes the empty string, and any other JSON value is
/// rendered via its `Display` impl (numbers, booleans) or JSON serialization
/// (objects, arrays). Missing keys collapse to the empty string so a
/// poorly-templated prompt degrades gracefully instead of leaking template
/// syntax to the LLM.
pub fn interpolate(template: &str, vars: &serde_json::Value) -> String {
    // Single left-to-right pass: emit template text and substitutions in order,
    // advancing past each replacement so substituted content is NEVER re-scanned.
    // Re-scanning the result (the previous behavior) let a value containing
    // `{{key}}` expand into another slot (cross-slot prompt injection) and let a
    // self-referential value (`{{x}}` -> `{{x}}`) loop forever.
    let mut result = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(end_offset) = rest[start..].find("}}") else {
            break;
        };
        let end = start + end_offset;
        result.push_str(&rest[..start]);
        let path = rest[start + 2..end].trim();
        let val = resolve_dot_path(vars, path);
        let replacement = match &val {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        };
        result.push_str(&replacement);
        rest = &rest[end + 2..];
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn interpolate_simple_string() {
        let vars = json!({ "name": "kleos" });
        assert_eq!(interpolate("hello {{name}}", &vars), "hello kleos");
    }

    #[test]
    fn interpolate_dot_path() {
        let vars = json!({ "agent": { "name": "claude" } });
        assert_eq!(interpolate("Agent: {{agent.name}}", &vars), "Agent: claude");
    }

    #[test]
    fn interpolate_missing_key_becomes_empty() {
        let vars = json!({ "name": "kleos" });
        assert_eq!(interpolate("{{missing}} bar", &vars), " bar");
    }

    #[test]
    fn interpolate_number_value() {
        let vars = json!({ "limit": 42 });
        assert_eq!(interpolate("limit={{limit}}", &vars), "limit=42");
    }

    #[test]
    fn interpolate_null_becomes_empty() {
        let vars = json!({ "x": null });
        assert_eq!(interpolate("[{{x}}]", &vars), "[]");
    }

    #[test]
    fn interpolate_unterminated_placeholder_left_intact() {
        let vars = json!({ "name": "k" });
        assert_eq!(interpolate("hello {{name", &vars), "hello {{name");
    }

    #[test]
    fn resolve_dot_path_missing_returns_null() {
        let obj = json!({ "a": { "b": 1 } });
        assert_eq!(resolve_dot_path(&obj, "a.c"), serde_json::Value::Null);
    }

    #[test]
    fn resolve_dot_path_traverses_objects() {
        let obj = json!({ "a": { "b": { "c": "deep" } } });
        assert_eq!(resolve_dot_path(&obj, "a.b.c"), json!("deep"));
    }

    /// A substituted value that itself contains `{{...}}` must be emitted
    /// verbatim, not re-expanded into another slot (cross-slot prompt
    /// injection). Here `payload` resolves to the literal text `{{secret}}`,
    /// which must NOT then be replaced by the value of `secret`.
    #[test]
    fn interpolate_does_not_reexpand_substituted_values() {
        let vars = json!({ "payload": "{{secret}}", "secret": "TOPSECRET" });
        assert_eq!(interpolate("note: {{payload}}", &vars), "note: {{secret}}");
    }

    /// A self-referential value must not loop forever; it is emitted once.
    #[test]
    fn interpolate_self_reference_terminates() {
        let vars = json!({ "x": "{{x}}" });
        assert_eq!(interpolate("{{x}}", &vars), "{{x}}");
    }
}
