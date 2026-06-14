//! MCP tool registry and dispatcher.
//!
//! The server route table contains both daily-driver tools and a very large
//! auto-generated long tail. `registry()` intentionally exposes only the
//! daily-use surface for MCP clients, while still deriving every entry from
//! `kleos_client::ROUTES` so schemas and descriptions stay source-aligned.

use crate::App;
use kleos_client::{find_by_name, resolve_tool_name, Route};
use serde_json::{json, Value};
use std::collections::HashSet;

/// The curated daily-driver tool names exposed through `tools/list`.
///
/// Canonical names and selected aliases both appear here when they are part
/// of the normal human workflow or preserve compatibility with existing MCP
/// client setups.
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
    "scratchpad.get",
    "prompts.generate",
    "context.generate_prompt",
    "prompts.header",
    "context.get_header",
    "mcp_schema.get",
    "errors.report",
    "agents.verify",
    // -- forge (agent-forge stateful operations) --
    "forge.spec_task",
    "forge_spec_task",
    "forge.update_spec",
    "forge_update_spec",
    "forge.list_specs",
    "forge_list_specs",
    "forge.get_spec",
    "forge_get_spec",
    "forge.log_hypothesis",
    "forge_log_hypothesis",
    "forge.log_outcome",
    "forge_log_outcome",
    "forge.recall_errors",
    "forge_recall_errors",
    "forge.consider_approaches",
    "forge_consider_approaches",
    "forge.verify",
    "forge_verify",
    "forge.session_learn",
    "forge_session_learn",
    "forge.session_recall",
    "forge_session_recall",
    // -- forge compute (stateless) --
    "forge.think",
    "forge_think",
    "forge.declare_unknowns",
    "forge_declare_unknowns",
    "forge.comment_check",
    "forge_comment_check",
    "forge.challenge_code",
    "forge_challenge_code",
    "forge.repo_map",
    "forge_repo_map",
    "forge.search_code",
    "forge_search_code",
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
/// Every tool is advertised under its underscore-normalized name
/// (`memory.store` -> `memory_store`) because strict MCP clients (VS Code)
/// reject dot-notation names and silently drop those tools. `tools/call`
/// resolves the underscore name back to the canonical route via
/// [`resolve_tool_name`]. `DAILY_TOOL_NAMES` lists some tools under both forms,
/// so entries are deduplicated by canonical route -- each tool appears once.
pub fn registry() -> Vec<Value> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for name in DAILY_TOOL_NAMES {
        let Some(route) = find_by_name(name) else {
            tracing::warn!(tool = %name, "daily MCP tool is missing from route registry");
            continue;
        };
        // Advertise the underscore form of THIS listed name (which may be a
        // back-compat alias, not the canonical route name), so curated aliases
        // stay visible. Dedup by the resulting display name collapses exact
        // dot/underscore pairs (`memory.store` + `memory_store`) to one entry.
        let display = name.replace('.', "_");
        if !seen.insert(display.clone()) {
            continue;
        }
        out.push(registry_entry(&display, route));
    }
    out
}

/// Routes an MCP tool call to the registered HTTP route. The arguments are
/// passed straight through; path templates extract the relevant fields.
#[tracing::instrument(skip(app, args), fields(name = %name))]
pub async fn dispatch(app: &App, name: &str, args: Value) -> Result<Value, String> {
    // Secret-bearing routes (e.g. cred resolve/proxy) are dispatchable by name
    // even though they are absent from the curated tools/list. Refuse them here
    // so a raw credential never reaches the MCP/model-context channel.
    if kleos_client::is_mcp_blocked(name) {
        return Err(format!("tool '{name}' is not available over MCP"));
    }
    let route = resolve_tool_name(name).ok_or_else(|| format!("unknown tool: {name}"))?;
    app.client.call_route(route, args).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every advertised tool name must be dot-free (VS Code's MCP validator
    /// rejects dots), unique, and resolvable back to its route so `tools/call`
    /// can dispatch whatever `tools/list` advertised.
    #[test]
    fn registry_names_are_underscore_unique_and_resolvable() {
        let tools = registry();
        assert!(!tools.is_empty(), "registry must not be empty");
        let mut seen = HashSet::new();
        for tool in &tools {
            let name = tool["name"].as_str().expect("tool has a name");
            assert!(
                !name.contains('.'),
                "tool name {name} contains a dot; VS Code will drop it"
            );
            assert!(seen.insert(name.to_string()), "duplicate tool name {name}");
            assert!(
                resolve_tool_name(name).is_some(),
                "advertised tool {name} does not resolve back to a route"
            );
        }
    }
}
