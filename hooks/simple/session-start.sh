#!/bin/bash
# Claude Code SessionStart hook - Simple version
# Bootstraps context from Kleos at session start.
# Requires: kleos-cli in PATH, KLEOS_URL and KLEOS_API_KEY env vars

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
KLEOS_CLI="${KLEOS_CLI:-$(command -v kleos-cli 2>/dev/null || echo '')}"
if [ -z "$KLEOS_CLI" ] || [ ! -f "$KLEOS_CLI" ]; then
  echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"kleos-cli not found. Install it or set KLEOS_CLI env var."}}'
  exit 0
fi

# Check required env vars
if [ -z "${KLEOS_URL:-}" ] || [ -z "${KLEOS_API_KEY:-}" ]; then
  echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"KLEOS_URL and KLEOS_API_KEY must be set."}}'
  exit 0
fi

# Get context from Kleos
CONTEXT=$("$KLEOS_CLI" context "agent-rules infrastructure active-tasks recent-decisions" --budget 3000 --quiet 2>/dev/null || echo "")

# Get recent memories
RECENT=$("$KLEOS_CLI" list --limit 5 --quiet 2>/dev/null | head -20 || echo "")

if [ -z "$CONTEXT" ] && [ -z "$RECENT" ]; then
  echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Kleos unreachable. Check KLEOS_URL and KLEOS_API_KEY."}}'
  exit 0
fi

# Build output
BLOCK="=== KLEOS CONTEXT ===
$CONTEXT

=== RECENT MEMORIES ===
$RECENT

=== RULES ===
Search Kleos BEFORE asking questions about servers, credentials, or past decisions.
Store outcomes to Kleos AFTER completing tasks."

ESCAPED=$(json_escape "$BLOCK")
printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$ESCAPED"
