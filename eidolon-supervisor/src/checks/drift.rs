use super::Violation;
use super::Severity;

pub fn check_promise_action(
    assistant_text: Option<&str>,
    tool_entry: &serde_json::Value,
) -> Vec<Violation> {
    // Placeholder for promise-vs-action drift detection.
    // Full implementation requires tracking assistant text from one turn
    // and comparing against tool calls in the next turn.
    // For MVP, this is a no-op -- the other checks provide immediate value.
    let _ = (assistant_text, tool_entry);
    Vec::new()
}
