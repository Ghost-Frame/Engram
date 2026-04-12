#!/bin/bash
# Claude Code UserPromptSubmit hook - Simple version
# Searches Kleos for relevant context based on user's message.

set -euo pipefail

# JSON escape helper
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
  exit 0
fi

# Check required env vars
if [ -z "${KLEOS_URL:-}" ] || [ -z "${KLEOS_API_KEY:-}" ]; then
  exit 0
fi

# Get user message from stdin (Claude Code passes it as JSON)
USER_INPUT=""
if [ -t 0 ]; then
  exit 0
else
  INPUT_JSON=$(cat)
  # Extract user message - simple grep approach
  USER_INPUT=$(echo "$INPUT_JSON" | grep -o '"content":"[^"]*"' | head -1 | sed 's/"content":"//;s/"$//' || echo "")
fi

if [ -z "$USER_INPUT" ] || [ ${#USER_INPUT} -lt 10 ]; then
  exit 0
fi

# Search Kleos for relevant memories (limit query length)
QUERY="${USER_INPUT:0:200}"
RESULTS=$("$KLEOS_CLI" search "$QUERY" --limit 3 --quiet 2>/dev/null | head -15 || echo "")

if [ -z "$RESULTS" ]; then
  exit 0
fi

BLOCK="Relevant Kleos memories:
$RESULTS"

ESCAPED=$(json_escape "$BLOCK")
printf '{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"%s"}}\n' "$ESCAPED"
