#!/bin/bash
# Claude Code Stop hook - Simple version
# Stores session summary to Engram when session ends.

set -euo pipefail

# Find engram-cli
ENGRAM_CLI="${ENGRAM_CLI:-$(command -v engram-cli 2>/dev/null || echo '')}"
if [ -z "$ENGRAM_CLI" ] || [ ! -f "$ENGRAM_CLI" ]; then
  exit 0
fi

# Check required env vars
if [ -z "${ENGRAM_URL:-}" ] || [ -z "${ENGRAM_API_KEY:-}" ]; then
  exit 0
fi

# Build summary from tool stats if available
SUMMARY="Session ended"
if [ -n "${CLAUDE_TOOL_STATS:-}" ]; then
  SUMMARY="Session activity: $CLAUDE_TOOL_STATS"
fi

# Store to Engram
"$ENGRAM_CLI" store "[session] $SUMMARY" --category session --quiet 2>/dev/null || true

exit 0
