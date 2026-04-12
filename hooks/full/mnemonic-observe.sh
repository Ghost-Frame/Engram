#!/bin/bash
# PostToolUse hook: notify engram-sidecar (Rust) of tool use (fire-and-forget)
# Reads tool_name + tool_input from stdin JSON, sends to engram-sidecar /observe.
# Returns empty (no hookSpecificOutput) to avoid context noise.
#
# The Rust engram-sidecar replaces the legacy Node.js mnemonic sidecar.
# It handles observe, recall, compress, and auto-capture in one binary.
#
# NOTE: Use absolute paths, not $HOME -- PostToolUse hooks don't expand $HOME on Windows.

SIDECAR_URL="${ENGRAM_SIDECAR_URL:-http://localhost:7711}"

# Check if sidecar is running; do NOT auto-start here (sidecar should be
# launched by session-start hook or systemd/launchd/process manager).
if ! curl -sf --max-time 1 "$SIDECAR_URL/health" >/dev/null 2>&1; then
  exit 0
fi

# Single python3 invocation: read stdin directly, extract fields, fire curl.
# Never capture stdin in a shell variable -- tool_response can be megabytes.
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
