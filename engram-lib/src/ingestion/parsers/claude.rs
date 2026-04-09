// ============================================================================
// Claude conversation export parser -- ported from parsers/claude.ts
// ============================================================================

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct ClaudeMessage {
    sender: String,
    text: String,
    #[allow(dead_code)]
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ClaudeConversation {
    uuid: String,
    name: String,
    created_at: String,
    updated_at: String,
    chat_messages: Vec<ClaudeMessage>,
}

/// Check if the JSON value is a Claude export format.
pub fn is_claude_export(value: &serde_json::Value) -> bool {
    if let Some(arr) = value.as_array() {
        if let Some(first) = arr.first() {
            if let Some(obj) = first.as_object() {
                return obj.contains_key("uuid") && obj.contains_key("chat_messages");
            }
        }
    }
    false
}

/// Build conversation text from messages.
fn build_text(messages: &[ClaudeMessage]) -> String {
    messages
        .iter()
        .map(|m| {
            let prefix = if m.sender == "human" {
                "Human"
            } else {
                "Assistant"
            };
            format!("{}: {}", prefix, m.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse Claude conversation export JSON into parsed documents.
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    let conversations: Vec<ClaudeConversation> = serde_json::from_str(input)?;
    let mut docs = Vec::new();

    for conv in &conversations {
        let text = build_text(&conv.chat_messages);

        let mut metadata = HashMap::new();
        metadata.insert(
            "uuid".to_string(),
            serde_json::Value::String(conv.uuid.clone()),
        );
        metadata.insert(
            "updated_at".to_string(),
            serde_json::Value::String(conv.updated_at.clone()),
        );

        docs.push(ParsedDocument {
            title: conv.name.clone(),
            text,
            metadata,
            source: "claude-export".to_string(),
            timestamp: Some(conv.created_at.clone()),
        });
    }

    Ok(docs)
}

/// Detect if input is a Claude export.
pub fn detect(input: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(val) => is_claude_export(&val),
        Err(_) => false,
    }
}
