use serde_json::Value;

/// Inject mode and agent context into a search request body before forwarding.
///
/// - If the request doesn't have a "mode" field, inject the session's active mode.
/// - Always inject "source" as the agent name if not already set.
pub fn inject_search_context(
    body: &mut Value,
    agent: &str,
    mode: &Option<String>,
) {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Inject mode if not present and session has one
    if !obj.contains_key("mode") {
        if let Some(ref m) = mode {
            obj.insert("mode".to_string(), Value::String(m.clone()));
        }
    }

    // Tag the request with the agent source if not already set
    if !obj.contains_key("agent") {
        obj.insert("agent".to_string(), Value::String(agent.to_string()));
    }
}

/// Inject agent/source metadata into a store request body before forwarding.
pub fn inject_store_context(body: &mut Value, agent: &str) {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // Set source to agent name if not already specified
    if !obj.contains_key("source") || obj.get("source").and_then(|v| v.as_str()) == Some("unknown") {
        obj.insert("source".to_string(), Value::String(agent.to_string()));
    }
}
