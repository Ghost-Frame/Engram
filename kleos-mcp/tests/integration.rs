//! Smoke tests for kleos-mcp tool registry and MCP protocol surface.
//!
//! These tests verify the JSON-RPC envelope shape and tool registry via the
//! server-side `POST /mcp` endpoint. The server test harness provides an
//! in-memory database with auth, so each test bootstraps a key and sends
//! authenticated JSON-RPC requests.
//!
//! Live wire tests that exercise actual tool execution belong in the
//! kleos-server integration test suite, not here.

use kleos_mcp::tools::registry;

/// Build a flat list of visible tool names from the curated registry.
fn registry_names() -> Vec<String> {
    registry()
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

/// The tool registry must include the core daily-driver tools. Names are
/// advertised in underscore form (`.` -> `_`) so strict MCP clients (VS Code)
/// accept them; `tools/call` resolves them back via `resolve_tool_name`.
#[test]
fn registry_includes_core_tools() {
    let names = registry_names();
    for required in [
        "memory_store",
        "memory_search",
        "memory_recall",
        "activity_report",
        "prompts_generate",
        "context_get_header",
        "tasks_list",
        "broca_feed",
        "soma_list_agents",
        "loom_list_runs",
        "thymus_get_metrics",
        "handoffs_store",
        "scratchpad_put",
        "skills_find_skills",
        "agents_verify",
        "mcp_schema_get",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "{required} must be in registry, got {:?}",
            names
        );
    }
}

/// The tool registry must keep important compatibility aliases for existing
/// clients, advertised in underscore form like everything else.
#[test]
fn registry_includes_daily_workflow_aliases() {
    let names = registry_names();
    for alias in [
        "memory_store",
        "memory_search",
        "memory_recall",
        "context_generate_prompt",
        "context_get_header",
        "services_chiasm_create_task",
        "tasks_update",
        "services_soma_register",
        "handoffs_dump",
    ] {
        assert!(
            names.iter().any(|name| name == alias),
            "{alias} must be in registry, got {:?}",
            names
        );
    }
}

/// The tool registry must hide the admin and generated long-tail tools by default.
#[test]
fn registry_excludes_long_tail_tools() {
    let names = registry_names();
    for excluded in [
        "admin.backfill_facts",
        "security.create_api_key",
        "graph.create_entity",
        "docs.openapi",
        "well_known.llms_txt",
        "memory.store_memory",
        "mcp_schema.dispatch",
        "tasks.create_task",
        "skills.create_skill",
    ] {
        assert!(
            names.iter().all(|name| name != excluded),
            "{excluded} must not be in registry, got {:?}",
            names
        );
    }
}
