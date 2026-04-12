#!/bin/bash
# PostToolUse hook: notify Mnemonic sidecar of tool use (fire-and-forget)
# Reads tool_name + tool_input from stdin JSON, sends to localhost:7711/observe.
# Auto-starts mnemonic if not running.
# Returns empty (no hookSpecificOutput) to avoid context noise.
#
# NOTE: Use absolute paths, not $HOME -- PostToolUse hooks don't expand $HOME on Windows.

# Auto-start mnemonic if not running
if ! curl -sf --max-time 1 http://localhost:7711/health >/dev/null 2>&1; then
  NODE_BIN="$(command -v node 2>/dev/null || echo "/c/Users/Zan/AppData/Roaming/fnm/node-versions/v24.14.1/installation/node")"
  "$NODE_BIN" --experimental-strip-types --no-warnings "/c/Users/Zan/.local/lib/mnemonic/index.ts" &disown 2>/dev/null
  sleep 0.5
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
        inp.get('description',
        inp.get('prompt', ''))))
    )[:200]
else:
    summary = ''

payload = json.dumps({'tool': tool, 'summary': summary})

try:
    subprocess.Popen(
        ['curl', '-sf', '--max-time', '1', 'http://localhost:7711/observe',
         '-X', 'POST', '-H', 'Content-Type: application/json', '-d', payload],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
except:
    pass
" 2>/dev/null

exit 0
