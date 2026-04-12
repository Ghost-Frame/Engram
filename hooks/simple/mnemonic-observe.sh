#!/bin/bash
# Claude Code PostToolUse hook - Mnemonic observer
# Reports tool usage to engram-sidecar for pattern learning.
# Requires: engram-sidecar running on localhost:7711

set -euo pipefail

MNEMONIC_URL="${MNEMONIC_URL:-http://localhost:7711}"

# Check if sidecar is running
if ! curl -sf --max-time 1 "$MNEMONIC_URL/health" > /dev/null 2>&1; then
  exit 0
fi

# Read tool use data from stdin
if [ -t 0 ]; then
  exit 0
fi

INPUT_JSON=$(cat)

# Forward to sidecar
curl -sf --max-time 2 \
  -X POST \
  -H "Content-Type: application/json" \
  -d "$INPUT_JSON" \
  "$MNEMONIC_URL/observe" > /dev/null 2>&1 || true

exit 0
