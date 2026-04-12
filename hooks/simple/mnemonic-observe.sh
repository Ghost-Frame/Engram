#!/bin/bash
# Claude Code PostToolUse hook - Mnemonic observer (Rust sidecar)
# Reports tool usage to engram-sidecar for pattern learning and auto-capture.
# Requires: engram-sidecar running on localhost:7711
# Accepts both legacy {tool, summary} and current {tool_name, content} formats.

set -euo pipefail

SIDECAR_URL="${ENGRAM_SIDECAR_URL:-http://localhost:7711}"

# Check if sidecar is running (fail-fast, no noise)
if ! curl -sf --max-time 1 "$SIDECAR_URL/health" > /dev/null 2>&1; then
  exit 0
fi

# Read tool use data from stdin -- never capture in a shell variable,
# tool_response can be megabytes.
if [ -t 0 ]; then
  exit 0
fi

# Use python3 to safely extract fields and fire a background curl.
python3 -c "
import sys, json, subprocess

try:
    data = json.load(sys.stdin)
except:
    sys.exit(0)

tool = data.get('tool_name', 'unknown')
inp = data.get('tool_input', {})

if isinstance(inp, str):
    summary = inp[:200]
elif isinstance(inp, dict):
    summary = str(
        inp.get('command',
        inp.get('file_path',
        inp.get('filePath',
        inp.get('description',
        inp.get('prompt', '')))))
    )[:200]
else:
    summary = ''

# Send in both legacy and current format for compatibility
payload = json.dumps({'tool': tool, 'tool_name': tool, 'summary': summary, 'content': summary})

try:
    subprocess.Popen(
        ['curl', '-sf', '--max-time', '2', '$SIDECAR_URL/observe',
         '-X', 'POST', '-H', 'Content-Type: application/json', '-d', payload],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
except:
    pass
" 2>/dev/null

exit 0
