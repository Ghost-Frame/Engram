//! MCP tool registry and dispatcher.
//!
//! The server route table contains both daily-driver tools and a very large
//! auto-generated long tail. `registry()` intentionally exposes only the
//! daily-use surface for MCP clients, while still deriving every entry from
//! `kleos_client::ROUTES` so schemas and descriptions stay source-aligned.
//!
//! All exposed names are normalised to underscore form (e.g. `memory_store`
//! rather than `memory.store`) because VS Code's MCP validator rejects tool
//! names containing dots. `tools/call` in lib.rs translates the underscore
//! name back to the canonical dot-name before forwarding to the server.

use crate::App;
use kleos_client::{find_by_name, Route};
use serde_json::{json, Value};
use std::collections::HashSet;

/// The curated daily-driver tool names exposed through `tools/list`.
///
/// Canonical dot-names and their underscore aliases both appear here so that
/// existing MCP client configurations that reference either form continue to
/// work. The `registry()` function normalises all entries to underscore form
/// and deduplicates, so each tool appears exactly once in `tools/list`.
const DAILY_TOOL_NAMES: &[&str] = &[
    "memory.store",
    "memory_store",
    "memory.search",
    "memory_search",
    "memory_search_preset",
    "memory.get",
    "memory.list",
    "memory_list",
    "memory.recall",
    "memory_recall",
    "skill.search",
    "skill_search",
    "skill.execute",
    "skill_execute",
    "skills.find_skills",
    "skills.usage_stats",
    "activity.report",
    "tasks.list",
    "tasks.create",
    "services.chiasm_create_task",
    "tasks.feed",
    "tasks.get_task",
    "tasks.update_task",
    "tasks.update",
    "services.chiasm_update_task",
    "broca.feed",
    "axon.list_events",
    "services.axon_consume",
    "soma.list_agents",
    "soma.create_agent",
    "soma.register",
    "services.soma_register",
    "loom.list_runs",
    "thymus.get_metrics",
    "handoffs.store",
    "handoffs.dump",
    "handoffs.list",
    "handoffs.latest",
    "handoffs.search",
    "sessions.get",
    "sessions.append",
    "sessions.list_sessions",
    "sessions.create_session",
    "sessions.stream",
    "scratchpad.list",
    "scratchpad.put",
    "scratchpad.delete_key",
    "scratchpad.delete_session",
    "scratchpad.promote",
    "prompts.generate",
    "context.generate_prompt",
    "prompts.header",
    "context.get_header",
    "mcp_schema.get",
    "errors.report",
    "agents.verify",
];

/// Parse one route's schema, falling back to an object-shaped schema on bad metadata.
fn route_schema(route: &Route) -> Value {
    serde_json::from_str(route.input_schema)
        .unwrap_or_else(|_| json!({ "type": "object", "additionalProperties": true }))
}

/// Build one MCP tool entry from the chosen visible tool name and backing route metadata.
fn registry_entry(name: &str, route: &Route) -> Value {
    json!({
        "name": name,
        "description": route.description,
        "inputSchema": route_schema(route),
    })
}

/// Returns the curated tool registry as JSON objects suitable for an MCP
/// `tools/list` response.
///
/// Every entry is emitted under its underscore-normalised name (`.` → `_`)
/// so VS Code's MCP client accepts it. Because `DAILY_TOOL_NAMES` lists both
/// the canonical dot-form and the underscore alias for some tools, entries are
/// deduplicated by resolved route so each tool appears exactly once.
pub fn registry() -> Vec<Value> {
    let mut seen: HashSet<String> = HashSet::new();
    DAILY_TOOL_NAMES
        .iter()
        .filter_map(|&name| {
            let route = find_by_name(name).or_else(|| {
                tracing::warn!(tool = %name, "daily MCP tool is missing from route registry");
                None
            })?;
            let emit_name = route.name.replace('.', "_");
            if seen.insert(emit_name.clone()) {
                Some(registry_entry(&emit_name, route))
            } else {
                None
            }
        })
        .collect()
}

/// Routes an MCP tool call to the registered HTTP route. The arguments are
/// passed straight through; path templates extract the relevant fields.
#[tracing::instrument(skip(app, args), fields(name = %name))]
pub async fn dispatch(app: &App, name: &str, args: Value) -> Result<Value, String> {
    let route = find_by_name(name).ok_or_else(|| format!("unknown tool: {name}"))?;
    app.client.call_route(route, args).await
}
