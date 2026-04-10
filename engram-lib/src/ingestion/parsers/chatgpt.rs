// ============================================================================
// ChatGPT conversation export parser -- ported from parsers/chatgpt.ts
// ============================================================================

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Deserialize)]
struct ChatGPTMessage {
    author: ChatGPTAuthor,
    content: ChatGPTContent,
    #[allow(dead_code)]
    create_time: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ChatGPTAuthor {
    role: String,
}

#[derive(Debug, Deserialize)]
struct ChatGPTContent {
    #[serde(default)]
    parts: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ChatGPTNode {
    message: Option<ChatGPTMessage>,
    parent: Option<String>,
    #[serde(default)]
    children: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ChatGPTConversation {
    title: String,
    create_time: f64,
    update_time: f64,
    mapping: HashMap<String, ChatGPTNode>,
}

/// Check if the JSON value is a ChatGPT export format.
pub fn is_chatgpt_export(value: &serde_json::Value) -> bool {
    if let Some(arr) = value.as_array() {
        if let Some(first) = arr.first() {
            if let Some(obj) = first.as_object() {
                return obj.contains_key("title") && obj.contains_key("mapping");
            }
        }
    }
    false
}

/// Find the root node (one with parent == null).
fn find_root(mapping: &HashMap<String, ChatGPTNode>) -> Option<String> {
    for (id, node) in mapping {
        if node.parent.is_none() {
            return Some(id.clone());
        }
    }
    None
}

/// Walk the conversation tree depth-first, collecting messages.
fn walk_tree(mapping: &HashMap<String, ChatGPTNode>, start_id: &str) -> Vec<(String, String)> {
    let mut messages: Vec<(String, String)> = Vec::new();
    let mut visited = HashSet::new();

    fn walk(
        mapping: &HashMap<String, ChatGPTNode>,
        node_id: &str,
        visited: &mut HashSet<String>,
        messages: &mut Vec<(String, String)>,
    ) {
        if visited.contains(node_id) {
            return;
        }
        visited.insert(node_id.to_string());

        let node = match mapping.get(node_id) {
            Some(n) => n,
            None => return,
        };

        if let Some(ref msg) = node.message {
            if msg.author.role != "system" && !msg.content.parts.is_empty() {
                let text: String = msg
                    .content
                    .parts
                    .iter()
                    .filter_map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                let text = text.trim().to_string();
                if !text.is_empty() {
                    let prefix = if msg.author.role == "user" {
                        "User"
                    } else {
                        "Assistant"
                    };
                    messages.push((prefix.to_string(), text));
                }
            }
        }

        for child_id in &node.children {
            walk(mapping, child_id, visited, messages);
        }
    }

    walk(mapping, start_id, &mut visited, &mut messages);
    messages
}

/// Build conversation text from messages.
fn build_text(messages: &[(String, String)]) -> String {
    messages
        .iter()
        .map(|(prefix, text)| format!("{}: {}", prefix, text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse ChatGPT conversation export JSON into parsed documents.
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    let conversations: Vec<ChatGPTConversation> = serde_json::from_str(input)?;
    let mut docs = Vec::new();

    for conv in &conversations {
        let root_id = find_root(&conv.mapping);
        let messages = match root_id {
            Some(ref id) => walk_tree(&conv.mapping, id),
            None => Vec::new(),
        };
        let text = build_text(&messages);

        let timestamp = chrono::DateTime::from_timestamp(conv.create_time as i64, 0)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default();

        let mut metadata = HashMap::new();
        metadata.insert(
            "update_time".to_string(),
            serde_json::Value::Number(
                serde_json::Number::from_f64(conv.update_time)
                    .unwrap_or(serde_json::Number::from(0)),
            ),
        );

        docs.push(ParsedDocument {
            title: conv.title.clone(),
            text,
            metadata,
            source: "chatgpt-export".to_string(),
            timestamp: Some(timestamp),
        });
    }

    Ok(docs)
}

/// Detect if input is a ChatGPT export.
pub fn detect(input: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(val) => is_chatgpt_export(&val),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_chatgpt() {
        let data = r#"[{"title": "Test", "create_time": 1700000000, "update_time": 1700000001, "mapping": {"root": {"message": null, "parent": null, "children": ["child1"]}, "child1": {"message": {"author": {"role": "user"}, "content": {"parts": ["Hello"]}, "create_time": 1700000000}, "parent": "root", "children": []}}}]"#;
        assert!(detect(data));
    }

    #[test]
    fn test_detect_not_chatgpt() {
        assert!(!detect(r#"[{"role": "user", "content": "hi"}]"#));
        assert!(!detect("not json at all"));
    }

    #[test]
    fn test_parse_chatgpt() {
        let data = r#"[{"title": "Test Chat", "create_time": 1700000000, "update_time": 1700000001, "mapping": {"root": {"message": null, "parent": null, "children": ["msg1", "msg2"]}, "msg1": {"message": {"author": {"role": "user"}, "content": {"parts": ["Hello there"]}, "create_time": 1700000000}, "parent": "root", "children": []}, "msg2": {"message": {"author": {"role": "assistant"}, "content": {"parts": ["Hi! How can I help?"]}, "create_time": 1700000001}, "parent": "root", "children": []}}}]"#;
        let docs = parse(data).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "Test Chat");
        assert!(docs[0].text.contains("User: Hello there"));
        assert!(docs[0].text.contains("Assistant: Hi! How can I help?"));
        assert_eq!(docs[0].source, "chatgpt-export");
        assert!(docs[0].timestamp.is_some());
    }

    #[test]
    fn test_parse_skips_system_messages() {
        let data = r#"[{"title": "T", "create_time": 1, "update_time": 1, "mapping": {"r": {"message": null, "parent": null, "children": ["s"]}, "s": {"message": {"author": {"role": "system"}, "content": {"parts": ["sys prompt"]}, "create_time": 1}, "parent": "r", "children": []}}}]"#;
        let docs = parse(data).unwrap();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].text.is_empty());
    }
}
