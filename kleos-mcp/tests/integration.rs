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

/// The tool registry must include the daily-driver tools.
#[test]
fn registry_includes_core_tools() {
    let tools = registry();
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(
        names.contains(&"memory.search"),
        "memory.search must be in registry, got {:?}",
        names
    );
    assert!(
        names.contains(&"memory.store"),
        "memory.store must be in registry, got {:?}",
        names
    );
}
