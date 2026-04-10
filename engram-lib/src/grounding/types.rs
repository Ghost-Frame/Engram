// GROUNDING TYPES - Tool execution framework (ported from TS grounding/types.ts)
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackendType {
    Mcp,
    Shell,
    Web,
    Gui,
    System,
}
impl BackendType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Shell => "shell",
            Self::Web => "web",
            Self::Gui => "gui",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ToolStatus {
    Success,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Connected,
    Disconnected,
    Connecting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPolicy {
    pub allow_shell_commands: bool,
    pub allow_network_access: bool,
    pub allow_file_access: bool,
    pub allowed_domains: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub sandbox_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub return_schema: Option<serde_json::Value>,
    pub usage_hint: Option<String>,
    pub latency_hint: Option<String>,
    pub backend_type: BackendType,
    pub security_policy: Option<SecurityPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub status: ToolStatus,
    pub content: serde_json::Value,
    pub error: Option<String>,
    pub execution_time_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub name: String,
    pub backend: BackendType,
    pub timeout_ms: Option<u64>,
    pub max_retries: Option<u32>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub backend: BackendType,
    pub status: SessionStatus,
    pub tools: Vec<String>,
    pub created_at: String,
    pub last_activity_at: String,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub timestamp: String,
    pub success: bool,
    pub execution_time_ms: u64,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolQualityRecord {
    pub tool_key: String,
    pub backend: String,
    pub server: String,
    pub tool_name: String,
    pub description_hash: String,
    pub total_calls: i64,
    pub total_successes: i64,
    pub total_failures: i64,
    pub avg_execution_ms: f64,
    pub llm_flagged_count: i64,
    pub quality_score: f64,
    pub last_execution_at: Option<String>,
}

pub fn build_tool_key(backend: &str, server: &str, tool_name: &str) -> String {
    format!("{}:{}:{}", backend, server, tool_name)
}

pub struct ParsedToolKey {
    pub backend: String,
    pub server: String,
    pub tool_name: String,
}
pub fn parse_tool_key(key: &str) -> ParsedToolKey {
    let parts: Vec<&str> = key.splitn(3, ':').collect();
    match parts.len() {
        3 => ParsedToolKey {
            backend: parts[0].to_string(),
            server: parts[1].to_string(),
            tool_name: parts[2].to_string(),
        },
        2 => ParsedToolKey {
            backend: parts[0].to_string(),
            server: "default".to_string(),
            tool_name: parts[1].to_string(),
        },
        _ => ParsedToolKey {
            backend: "unknown".to_string(),
            server: "default".to_string(),
            tool_name: key.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_build_tool_key() {
        assert_eq!(
            build_tool_key("shell", "default", "exec"),
            "shell:default:exec"
        );
    }
    #[test]
    fn test_parse_tool_key_3() {
        let p = parse_tool_key("shell:srv:tool");
        assert_eq!(p.backend, "shell");
        assert_eq!(p.server, "srv");
        assert_eq!(p.tool_name, "tool");
    }
    #[test]
    fn test_parse_tool_key_2() {
        let p = parse_tool_key("shell:tool");
        assert_eq!(p.server, "default");
    }
    #[test]
    fn test_parse_tool_key_1() {
        let p = parse_tool_key("tool");
        assert_eq!(p.backend, "unknown");
    }
}
