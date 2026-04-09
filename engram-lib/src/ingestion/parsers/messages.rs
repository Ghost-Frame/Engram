// ============================================================================
// Generic message format parser -- ported from parsers/messages.ts
// ============================================================================

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct MessageEntry {
    role: String,
    content: String,
    #[allow(dead_code)]
    timestamp: Option<String>,
}

/// Capitalize the first character of a string.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => {
            let upper: String = c.to_uppercase().collect();
            format!("{}{}", upper, chars.collect::<String>())
        }
    }
}

/// Detect if input is a generic message format (JSON array of {role, content}).
pub fn detect(input: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(val) => {
            if let Some(arr) = val.as_array() {
                if let Some(first) = arr.first() {
                    if let Some(obj) = first.as_object() {
                        return obj.contains_key("role")
                            && obj.contains_key("content")
                            && obj.get("role").and_then(|v| v.as_str()).is_some()
                            && obj.get("content").and_then(|v| v.as_str()).is_some();
                    }
                }
            }
            false
        }
        Err(_) => false,
    }
}

/// Parse a JSON array of messages into a single parsed document.
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    let messages: Vec<MessageEntry> = serde_json::from_str(input)?;

    let parts: Vec<String> = messages
        .iter()
        .map(|m| format!("{}: {}", capitalize(&m.role), m.content))
        .collect();
    let full_text = parts.join("\n\n");
    let title: String = full_text.chars().take(60).collect();

    let mut metadata = HashMap::new();
    metadata.insert(
        "message_count".to_string(),
        serde_json::Value::Number(serde_json::Number::from(messages.len())),
    );

    Ok(vec![ParsedDocument {
        title,
        text: full_text,
        metadata,
        source: "messages".to_string(),
        timestamp: None,
    }])
}
