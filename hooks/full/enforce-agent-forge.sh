#!/bin/bash
# PreToolUse gate for Write/Edit: blocks file modifications unless agent-forge
# spec_task (new code) or log_hypothesis (bug fix) has been called this session.
#
# Bypass: trivial edits to non-code files pass through.
# Manual bypass: touch /tmp/claude-forge-bypass (cleared on session start).
# Exit 0 = allow, Exit 2 = block (stderr shown to Claude).

set -uo pipefail

INPUT=$(cat)

resolve_home() {
  if [ -n "${HOME:-}" ]; then printf '%s\n' "$HOME"; return; fi
  if command -v cygpath >/dev/null 2>&1 && [ -n "${USERPROFILE:-}" ]; then
    cygpath -u "$USERPROFILE"; return
  fi
  printf '%s\n' "${USERPROFILE:-.}"
}

HOME_DIR="$(resolve_home)"
STATE_DIR="$HOME_DIR/.claude/session-env"
LOG_DIR="$HOME_DIR/.claude/logs"
FORGE_STATE="$STATE_DIR/agent-forge-active"
BYPASS_FILE="/tmp/claude-forge-bypass"
LOG_FILE="$LOG_DIR/enforce-agent-forge.log"
mkdir -p "$LOG_DIR" 2>/dev/null || true

log() {
  echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*" >> "$LOG_FILE" 2>/dev/null || true
}

# Parse tool input
TOOL_NAME=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    print(d.get('tool_name', 'unknown'))
except: print('unknown')
" "$INPUT" 2>/dev/null || echo "unknown")

FILE_PATH=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    ti = d.get('tool_input', {})
    print(ti.get('file_path', ''))
except: print('')
" "$INPUT" 2>/dev/null || echo "")

log "Gate check: tool=$TOOL_NAME file=$FILE_PATH"

# --- Allow non-code files without forge ---
# Config, docs, markdown, JSON, YAML, TOML, lock files, .env
if echo "$FILE_PATH" | grep -qiE '\.(md|txt|json|yaml|yml|toml|lock|env|cfg|ini|conf|csv|xml|html|css|svg|sh)$'; then
  log "Allowed: non-code file $FILE_PATH"
  exit 0
fi

# Allow files in hooks directory (meta-editing)
if echo "$FILE_PATH" | grep -qiE '\.claude/hooks/'; then
  log "Allowed: hooks directory $FILE_PATH"
  exit 0
fi

# Allow CLAUDE.md and similar agent config
if echo "$FILE_PATH" | grep -qiE '(CLAUDE|AGENTS|GEMINI|README)\.md$'; then
  log "Allowed: agent config file $FILE_PATH"
  exit 0
fi

# --- Check bypass ---
if [ -f "$BYPASS_FILE" ]; then
  log "Allowed: manual bypass active"
  exit 0
fi

# --- Check agent-forge state ---
if [ -f "$FORGE_STATE" ]; then
  FORGE_TYPE=$(cat "$FORGE_STATE" 2>/dev/null || echo "unknown")
  log "Allowed: agent-forge active ($FORGE_TYPE)"
  exit 0
fi

# --- Eidolon gate check (defense in depth + session tracking) ---
. "$HOME_DIR/.claude/hooks/lib-eidolon.sh"
ESCAPED_PATH=$(json_escape "$FILE_PATH")
GATE_RESPONSE=$(eidolon_call POST "/gate/check" \
  "{\"tool_name\":\"$TOOL_NAME\",\"tool_input\":{\"file_path\":$ESCAPED_PATH}}" \
  3 || echo "")

if [ -n "$GATE_RESPONSE" ]; then
  GATE_ACTION=$(python3 -c "
import sys, json
try: print(json.loads(sys.argv[1]).get('action', 'allow'))
except: print('allow')
" "$GATE_RESPONSE" 2>/dev/null || echo "allow")

  if [ "$GATE_ACTION" = "block" ]; then
    GATE_MSG=$(python3 -c "
import sys, json
try: print(json.loads(sys.argv[1]).get('message', 'Blocked by Eidolon'))
except: print('Blocked by Eidolon')
" "$GATE_RESPONSE" 2>/dev/null || echo "Blocked by Eidolon")
    log "EIDOLON GATE BLOCKED Write/Edit: $GATE_MSG"
    echo "EIDOLON GATE DENIED: $GATE_MSG" >&2
    exit 2
  fi
fi

# --- BLOCKED ---
log "BLOCKED: no agent-forge state for code file $FILE_PATH"
echo "BLOCKED: Agent-forge protocol required before editing code files." >&2
echo "" >&2
echo "You MUST call one of these first:" >&2
echo "  - agent-forge spec-task (for new code)" >&2
echo "  - agent-forge log-hypothesis (for bug fixes)" >&2
echo "" >&2
echo "Target file: $FILE_PATH" >&2
echo "" >&2
echo "If this is truly a trivial edit, tell Master and create bypass:" >&2
echo "  touch /tmp/claude-forge-bypass" >&2
exit 2
