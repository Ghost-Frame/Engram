# Kleos MCP Client Setup

Known-good client setup reference for connecting Kleos to local MCP-capable
agent clients.

This document is intentionally opinionated. The primary recommended path is the
local `kleos-mcp` stdio bridge, because it matches the current Kleos auth and
transport model:

- `kleos-mcp` runs locally as the MCP server your client launches.
- `kleos-mcp` forwards JSON-RPC requests to the server-side `POST /mcp`
  endpoint.
- `kleos-mcp` authenticates onward requests with either a local signing
  identity or `KLEOS_API_KEY` bearer fallback.

If you only need the available tool names and groups, read
[`MCP_DAILY_TOOLS.md`](./MCP_DAILY_TOOLS.md). If you need the binary and CLI
reference, read [`KLEOS_OPERATIONS_MANUAL.md`](./KLEOS_OPERATIONS_MANUAL.md).

## Recommended model

Use the local stdio bridge first.

Do this:

- Point your client at the `kleos-mcp` binary.
- Set `KLEOS_URL` so the bridge knows which Kleos server to call.
- Prefer a local signing identity.
- Use `KLEOS_API_KEY` only as a fallback when the client cannot rely on local
  identity-based auth.

Avoid this unless you have a specific reason:

- Pointing the client directly at the server-side `/mcp` endpoint.
- Treating `kleos-mcp` like a separate tool implementation layer.
- Assuming every MCP client supports the same remote auth flow.

## Prerequisites

Before configuring any client:

1. `kleos-mcp` must be installed and on `PATH`.
2. The target Kleos server must be reachable at `KLEOS_URL`.
3. One auth path must exist:
   - preferred: local identity configured via `kleos-cli identity init`
   - fallback: `KLEOS_API_KEY`

Useful checks:

```bash
kleos-cli health
kleos-mcp --help
```

If `kleos-mcp` starts and immediately says it has no auth configured, fix auth
first. The bridge intentionally refuses to run unauthenticated.

## Shared env block

These env vars work well across clients:

```json
{
  "KLEOS_URL": "http://127.0.0.1:4200",
  "KLEOS_AGENT_LABEL": "your-client-name"
}
```

Optional bearer fallback:

```json
{
  "KLEOS_API_KEY": "your-api-key"
}
```

Only set `KLEOS_API_KEY` if you are intentionally using bearer fallback instead
of local signing identity.

## Claude Code

Claude Code supports project-scoped MCP config in `.mcp.json`.

Known-good project config:

```json
{
  "mcpServers": {
    "kleos": {
      "command": "kleos-mcp",
      "args": [],
      "env": {
        "KLEOS_URL": "http://127.0.0.1:4200",
        "KLEOS_AGENT_LABEL": "claude-code"
      }
    }
  }
}
```

If you need bearer fallback:

```json
{
  "mcpServers": {
    "kleos": {
      "command": "kleos-mcp",
      "args": [],
      "env": {
        "KLEOS_URL": "${KLEOS_URL:-http://127.0.0.1:4200}",
        "KLEOS_AGENT_LABEL": "claude-code",
        "KLEOS_API_KEY": "${KLEOS_API_KEY}"
      }
    }
  }
}
```

Notes:

- `.mcp.json` is the right project-local file for team-shared config.
- Claude Code will ask for approval before using project-scoped MCP servers.
- The local bridge is the cleanest path here. It avoids stuffing direct remote
  MCP auth details into the client config.

## Cursor

Cursor supports project config in `.cursor/mcp.json` and global config in
`~/.cursor/mcp.json`.

Known-good project config:

```json
{
  "mcpServers": {
    "kleos": {
      "command": "kleos-mcp",
      "args": [],
      "env": {
        "KLEOS_URL": "http://127.0.0.1:4200",
        "KLEOS_AGENT_LABEL": "cursor"
      }
    }
  }
}
```

Bearer fallback variant:

```json
{
  "mcpServers": {
    "kleos": {
      "command": "kleos-mcp",
      "args": [],
      "env": {
        "KLEOS_URL": "http://127.0.0.1:4200",
        "KLEOS_AGENT_LABEL": "cursor",
        "KLEOS_API_KEY": "${KLEOS_API_KEY}"
      }
    }
  }
}
```

Notes:

- Cursor supports multiple MCP transports, but the Kleos local stdio bridge is
  the least fragile setup.
- If you later expose Kleos through a trusted remote MCP endpoint, treat that as
  a separate deployment path and document it separately. Do not silently swap
  local bridge config for remote auth assumptions.

## OpenCode

OpenCode supports global config in `~/.config/opencode/opencode.json` and
project config in `opencode.json`.

Known-good project config:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "kleos": {
      "type": "local",
      "command": ["kleos-mcp"],
      "enabled": true,
      "environment": {
        "KLEOS_URL": "http://127.0.0.1:4200",
        "KLEOS_AGENT_LABEL": "opencode"
      }
    }
  }
}
```

Bearer fallback variant:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "kleos": {
      "type": "local",
      "command": ["kleos-mcp"],
      "enabled": true,
      "environment": {
        "KLEOS_URL": "http://127.0.0.1:4200",
        "KLEOS_AGENT_LABEL": "opencode",
        "KLEOS_API_KEY": "{env:KLEOS_API_KEY}"
      }
    }
  }
}
```

Optional direct remote pattern for OpenCode:

Use this only if you explicitly want OpenCode to talk to a remotely reachable
Kleos `/mcp` endpoint without the local bridge.

First mint a token intended for static-header MCP clients:

```bash
kleos-cli mcp-token mint --name opencode --scopes read,write
```

Then configure a remote MCP entry:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "kleos_remote": {
      "type": "remote",
      "url": "https://kleos.example.com/mcp",
      "enabled": true,
      "headers": {
        "Authorization": "Bearer {env:KLEOS_MCP_TOKEN}"
      }
    }
  }
}
```

Notes:

- Prefer the local bridge unless you specifically need remote transport.
- For direct remote mode, use an MCP token, not a hand-waved generic header.
- The remote endpoint must actually be reachable, and server-side MCP must not
  be disabled with `KLEOS_MCP_ENABLED=0`.

## Troubleshooting

### `kleos-mcp failed to start: no auth configured`

Cause:

- no local signing identity
- no `KLEOS_API_KEY`

Fix:

- run `kleos-cli identity init`, or
- export `KLEOS_API_KEY` for the client process

### Client launches the bridge but no tools appear

Check:

- `kleos-mcp` is on `PATH`
- `KLEOS_URL` points to the right server
- `kleos-cli health` succeeds
- the server-side MCP endpoint is enabled

### Direct remote mode works in one client and fails in another

That usually means you crossed transport/auth models.

Check:

- whether the client is using local stdio or remote HTTP
- whether you supplied a local bridge config or a direct remote config
- whether the remote path expects static bearer headers, OAuth, or something
  else entirely

Do not assume a config block from Claude Code, Cursor, and OpenCode is
interchangeable. The shapes differ even when the underlying MCP server is the
same.

## References

- Claude Code MCP docs: <https://code.claude.com/docs/en/mcp>
- Cursor MCP docs: <https://docs.cursor.com/context/model-context-protocol>
- OpenCode MCP docs: <https://opencode.ai/docs/mcp-servers>
- OpenCode config docs: <https://opencode.ai/docs/config>
