#!/bin/bash
# Claude Code SessionStart hook - Simple version
# Bootstraps context from Engram at session start.
# Requires: kleos-cli in PATH, ENGRAM_URL and ENGRAM_API_KEY env vars

set -euo pipefail

# Skip for subagents
if [ -n "${CLAUDE_CODE_ENTRYPOINT:-}" ] && [ "$CLAUDE_CODE_ENTRYPOINT" != "cli" ]; then
  exit 0
fi

# JSON escape helper (pure bash, handles common cases)
json_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  s="${s//$'\r'/\\r}"
  s="${s//$'\t'/\\t}"
  printf '%s' "$s"
}

# Find kleos-cli
ENGRAM_CLI="${ENGRAM_CLI:-$(command -v kleos-cli 2>/dev/null || echo '')}"
if [ -z "$ENGRAM_CLI" ] || [ ! -f "$ENGRAM_CLI" ]; then
  echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"kleos-cli not found. Install it or set ENGRAM_CLI env var."}}'
  exit 0
fi

# Check required env vars
if [ -z "${ENGRAM_URL:-}" ] || [ -z "${ENGRAM_API_KEY:-}" ]; then
  echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"ENGRAM_URL and ENGRAM_API_KEY must be set."}}'
  exit 0
fi

# Get context from Engram
CONTEXT=$("$ENGRAM_CLI" context "agent-rules infrastructure active-tasks recent-decisions" --budget 3000 --quiet 2>/dev/null || echo "")

# Get recent memories
RECENT=$("$ENGRAM_CLI" list --limit 5 --quiet 2>/dev/null | head -20 || echo "")

if [ -z "$CONTEXT" ] && [ -z "$RECENT" ]; then
  echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Engram unreachable. Check ENGRAM_URL and ENGRAM_API_KEY."}}'
  exit 0
fi

# Build output
BLOCK="=== ENGRAM CONTEXT ===
$CONTEXT

=== RECENT MEMORIES ===
$RECENT

=== RULES ===
Search Engram BEFORE asking questions about servers, credentials, or past decisions.
Store outcomes to Engram AFTER completing tasks."

ESCAPED=$(json_escape "$BLOCK")
printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$ESCAPED"
