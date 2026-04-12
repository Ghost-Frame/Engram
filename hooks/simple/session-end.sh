#!/bin/bash
# Claude Code Stop hook - Simple version
# Stores session summary to Kleos when session ends.

set -euo pipefail

# Find kleos-cli
KLEOS_CLI="${KLEOS_CLI:-$(command -v kleos-cli 2>/dev/null || echo '')}"
if [ -z "$KLEOS_CLI" ] || [ ! -f "$KLEOS_CLI" ]; then
  exit 0
fi

# Check required env vars
if [ -z "${KLEOS_URL:-}" ] || [ -z "${KLEOS_API_KEY:-}" ]; then
  exit 0
fi

# Build summary from tool stats if available
SUMMARY="Session ended"
if [ -n "${CLAUDE_TOOL_STATS:-}" ]; then
  SUMMARY="Session activity: $CLAUDE_TOOL_STATS"
fi

# Store to Kleos
"$KLEOS_CLI" store "[session] $SUMMARY" --category session --quiet 2>/dev/null || true

exit 0
