// GROUNDING CLIENT - Provider registry and orchestration (ported from TS grounding/client.ts)
use super::types::*;
use super::shell::ShellProvider;
use std::collections::HashMap;
use tracing::{info, warn};

pub struct GroundingClient {
    shell: ShellProvider,
    tool_cache: Vec<ToolSchema>,
}

impl GroundingClient {
    pub fn new() -> Self {
        let shell = ShellProvider::new("default");
        let tool_cache = super::shell::shell_tools();
        info!("grounding client initialized with shell provider");
        Self { shell, tool_cache }
    }

    pub fn get_all_tools(&self) -> &[ToolSchema] { &self.tool_cache }

    pub async fn execute_tool(&self, tool_name: &str, args: &serde_json::Value, timeout_ms: Option<u64>) -> ToolResult {
        self.shell.execute_tool(tool_name, args, timeout_ms).await
    }

    pub fn create_session(&mut self, config: &SessionConfig) -> SessionInfo { self.shell.create_session(config) }
    pub fn destroy_session(&mut self, id: &str) { self.shell.destroy_session(id) }
    pub fn list_sessions(&self) -> Vec<&SessionInfo> { self.shell.list_sessions() }
}
