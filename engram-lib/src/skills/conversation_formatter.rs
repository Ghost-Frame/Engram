use serde::{Deserialize, Serialize};

// -- Priority levels --
pub const P_USER_INSTRUCTION: u8 = 0;
pub const P_SYSTEM: u8 = 1;
pub const P_RECENT_USER: u8 = 2;
pub const P_RECENT_ASSISTANT: u8 = 3;
pub const P_TOOL_RESULT: u8 = 4;
pub const P_OLDER_USER: u8 = 5;
pub const P_OLDER_ASSISTANT: u8 = 6;
pub const P_SKIP: u8 = 99;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub priority: Option<u8>,
}

/// Assign priority to a message based on role and position.
pub fn assign_priority(msg: &ConversationMessage, index: usize, total: usize) -> u8 {
    if let Some(p) = msg.priority { return p; }
    let is_recent = index >= total.saturating_sub(4);
    match msg.role.as_str() {
        "system" => P_SYSTEM,
        "user" if is_recent => P_RECENT_USER,
        "user" => P_OLDER_USER,
        "assistant" if is_recent => P_RECENT_ASSISTANT,
        "assistant" => P_OLDER_ASSISTANT,
        "tool" | "tool_result" => P_TOOL_RESULT,
        _ => P_SKIP,
    }
}

/// Truncate content by splitting in half with a marker.
pub fn truncate_content(content: &str, max_len: usize) -> String {
    if content.len() <= max_len { return content.to_string(); }
    let half = max_len / 2;
    let start = &content[..half];
    let end = &content[content.len() - half..];
    format!("{}\n[... truncated ...]\n{}", start, end)
}

/// Format a conversation with priority-based truncation.
pub fn format_conversation(messages: &[ConversationMessage], max_chars: usize) -> Vec<ConversationMessage> {
    let total = messages.len();
    let mut scored: Vec<(u8, usize)> = messages.iter().enumerate()
        .map(|(i, m)| (assign_priority(m, i, total), i))
        .filter(|(p, _)| *p != P_SKIP)
        .collect();
    scored.sort_by_key(|(p, _)| *p);

    let mut result = Vec::new();
    let mut char_budget = max_chars;
    for (_priority, idx) in &scored {
        let msg = &messages[*idx];
        if char_budget == 0 { break; }
        let truncated_content = if msg.content.len() > char_budget {
            truncate_content(&msg.content, char_budget)
        } else { msg.content.clone() };
        char_budget = char_budget.saturating_sub(truncated_content.len());
        result.push(ConversationMessage {
            role: msg.role.clone(),
            content: truncated_content,
            priority: Some(assign_priority(msg, *idx, total)),
        });
    }
    // Re-sort by original index
    result.sort_by_key(|m| {
        messages.iter().position(|orig| orig.content == m.content && orig.role == m.role).unwrap_or(0)
    });
    result
}

/// Format tool execution results into a readable string.
pub fn format_tool_results(tool_name: &str, success: bool, output: &str, duration_ms: Option<f64>) -> String {
    let status = if success { "SUCCESS" } else { "FAILED" };
    let dur = duration_ms.map(|d| format!(" ({:.0}ms)", d)).unwrap_or_default();
    format!("[{} {}{}]\n{}", tool_name, status, dur, output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_priority_system() {
        let msg = ConversationMessage { role: "system".into(), content: "hi".into(), priority: None };
        assert_eq!(assign_priority(&msg, 0, 5), P_SYSTEM);
    }
    #[test] fn test_priority_recent_user() {
        let msg = ConversationMessage { role: "user".into(), content: "hi".into(), priority: None };
        assert_eq!(assign_priority(&msg, 4, 5), P_RECENT_USER);
    }
    #[test] fn test_priority_older_user() {
        let msg = ConversationMessage { role: "user".into(), content: "hi".into(), priority: None };
        assert_eq!(assign_priority(&msg, 0, 10), P_OLDER_USER);
    }
    #[test] fn test_truncate_short() { assert_eq!(truncate_content("hello", 100), "hello"); }
    #[test] fn test_truncate_long() {
        let long: String = "a".repeat(200);
        let truncated = truncate_content(&long, 50);
        assert!(truncated.contains("[... truncated ...]"));
    }
    #[test] fn test_format_tool_success() {
        let result = format_tool_results("read_file", true, "content", Some(42.0));
        assert!(result.contains("SUCCESS"));
    }
    #[test] fn test_format_tool_failure() {
        let result = format_tool_results("write_file", false, "denied", None);
        assert!(result.contains("FAILED"));
    }
}