// ============================================================================
// JSONL line-by-line parser -- ported from parsers/jsonl.ts
// ============================================================================

use crate::ingestion::types::ParsedDocument;
use crate::Result;
use std::collections::HashMap;

const CONTENT_FIELDS: &[&str] = &["content", "text", "body", "message"];

/// Parse JSONL (newline-delimited JSON) into parsed documents.
pub fn parse(input: &str) -> Result<Vec<ParsedDocument>> {
    let mut docs = Vec::new();

    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let obj = match parsed.as_object() {
            Some(o) => o,
            None => continue,
        };

        // Find content field
        let mut content_field: Option<&str> = None;
        let mut content_value: Option<&str> = None;
        for field in CONTENT_FIELDS {
            if let Some(val) = obj.get(*field) {
                if let Some(s) = val.as_str() {
                    content_field = Some(field);
                    content_value = Some(s);
                    break;
                }
            }
        }

        let (content_field, content_value) = match (content_field, content_value) {
            (Some(f), Some(v)) => (f, v),
            _ => continue,
        };

        // Build title
        let title = obj
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| content_value.chars().take(60).collect());

        // Build metadata (all fields except content field)
        let mut metadata = HashMap::new();
        for (key, value) in obj {
            if key == content_field {
                continue;
            }
            metadata.insert(key.clone(), value.clone());
        }

        docs.push(ParsedDocument {
            title,
            text: content_value.to_string(),
            metadata,
            source: "jsonl".to_string(),
            timestamp: None,
        });
    }

    Ok(docs)
}

/// Detect if input has .jsonl extension.
pub fn detect(extension: Option<&str>) -> bool {
    extension
        .map(|e| e.to_lowercase() == ".jsonl")
        .unwrap_or(false)
}
