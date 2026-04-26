use super::Violation;
use super::Severity;

pub fn check_file_scope(entry: &serde_json::Value, allowed_paths: &[String]) -> Vec<Violation> {
    if allowed_paths.is_empty() {
        return Vec::new();
    }

    let file_path = match extract_file_path(entry) {
        Some(p) => p,
        None => return Vec::new(),
    };

    let in_scope = allowed_paths.iter().any(|allowed| file_path.starts_with(allowed));

    if !in_scope {
        return vec![Violation {
            rule_id: "scope-violation".into(),
            severity: Severity::Warning,
            message: format!("Edit outside allowed scope: {}", file_path),
            context: file_path,
        }];
    }

    Vec::new()
}

fn extract_file_path(entry: &serde_json::Value) -> Option<String> {
    let obj = entry.as_object()?;

    let tool_name = obj
        .get("tool_name")
        .or(obj.get("name"))
        .and_then(|v| v.as_str())?;

    if !matches!(tool_name, "Edit" | "Write" | "NotebookEdit") {
        return None;
    }

    let input = obj.get("tool_input").or(obj.get("input"))?;
    input
        .get("file_path")
        .or(input.get("filePath"))
        .and_then(|v| v.as_str())
        .map(String::from)
}
