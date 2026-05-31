## Kleos MCP Daily Tools

This document describes the curated daily-use MCP surface exposed by `kleos-mcp`.

It is intentionally smaller than the full `kleos-client::ROUTES` table. The goal is
to expose the tools agents should use constantly while hiding admin, generated, and
low-value maintenance routes from normal MCP clients.

If you need client-specific configuration examples for Claude Code, Cursor, or
OpenCode, read [`MCP_CLIENT_SETUP.md`](./MCP_CLIENT_SETUP.md).

### Transport model

- `kleos-mcp` is a thin stdio/HTTP bridge that forwards JSON-RPC requests to the
  server-side `POST /mcp` endpoint.
- The server-side endpoint owns dispatch, auth, scope checks, rate limiting,
  audit logging, and the tool registry.
- The standalone `kleos-mcp` binary must have either a local signing identity
  or `KLEOS_API_KEY` bearer fallback configured before it will start.

### Important behavior

- Use `POST /mcp` for normal MCP `tools/list` and `tools/call` traffic.
- Use `GET /mcp/schema` to inspect the currently exposed MCP tool schema.
- `GET /mcp/schema` is auth-gated like the rest of the authenticated API
  routes; it is not a public discovery endpoint.
- Prefer dotted tool names as the canonical form. Underscore and
  `services.*` spellings exist as compatibility aliases for older client setups.
- The server-side `POST /mcp` endpoint can be disabled with `KLEOS_MCP_ENABLED=0`.
- If the optional `kleos-mcp` HTTP transport is compiled in, it does not add
  client auth at the front door. Bind it only to loopback or another trusted
  private network boundary.
- Do not rely on `mcp_schema.dispatch`.
  That helper path was intentionally removed from the server and is no longer
  part of the advertised route metadata.

### Daily-use tool groups

#### Memory

- `memory.store`
- `memory.search`
- `memory.get`
- `memory.list`
- `memory.recall`

Compatibility aliases:

- `memory_store`
- `memory_search`
- `memory_search_preset`
- `memory_list`
- `memory_recall`

#### Skills

- `skill.search`
- `skill.execute`
- `skills.find_skills`
- `skills.usage_stats`

Compatibility aliases:

- `skill_search`
- `skill_execute`

#### Activity and errors

- `activity.report`
- `errors.report`

#### Task coordination

- `tasks.list`
- `tasks.create`
- `tasks.feed`
- `tasks.get_task`
- `tasks.update_task`

Compatibility aliases:

- `services.chiasm_create_task`
- `tasks.update`
- `services.chiasm_update_task`

#### Coordination feeds and agent presence

- `broca.feed`
- `axon.list_events`
- `soma.list_agents`
- `soma.create_agent`
- `loom.list_runs`
- `thymus.get_metrics`

Compatibility aliases:

- `services.axon_consume`
- `soma.register`
- `services.soma_register`

#### Handoffs

- `handoffs.store`
- `handoffs.list`
- `handoffs.latest`
- `handoffs.search`

Compatibility alias:

- `handoffs.dump`

#### Sessions and scratchpad

- `sessions.get`
- `sessions.append`
- `sessions.list_sessions`
- `sessions.create_session`
- `sessions.stream`
- `scratchpad.list`
- `scratchpad.put`
- `scratchpad.delete_key`
- `scratchpad.delete_session`
- `scratchpad.promote`

#### Context and prompts

- `prompts.generate`
- `prompts.header`

Compatibility aliases:

- `context.generate_prompt`
- `context.get_header`

#### Discovery and verification

- `mcp_schema.get`
- `agents.verify`

### What is intentionally excluded

The curated MCP surface hides these categories by default:

- Admin and tenant-management routes
- Auto-generated long-tail endpoints
- Graph and maintenance internals
- GUI, docs, and well-known endpoints
- Stale helper routes such as `mcp_schema.dispatch`

### Source of truth

The curated list is implemented in:

- `kleos-mcp/src/tools.rs`

Regression coverage lives in:

- `kleos-mcp/tests/integration.rs`
- `kleos-client/src/routes.rs`
