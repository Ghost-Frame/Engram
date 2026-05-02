#!/bin/bash
# PreToolUse gate: blocks ALL tool calls until kleos-cli search has been
# executed at least once this session.
#
# How it works:
#   - Session start clears the stamp file
#   - When a Bash command containing "kleos-cli search" (or the legacy
#     engram-cli alias) runs, the PostToolUse hook (or this hook itself on
#     pass-through) sets the stamp
#   - Until the stamp exists, all non-exempt tool calls are BLOCKED
#
# Exempt tools/commands (always allowed through):
#   - kleos-cli / engram-cli (legacy alias) / cred / echo / cat / ls / pwd / which / test (bootstrap)
#   - Read / Grep / Glob (read-only, needed to orient)
#   - Skill / ToolSearch (meta-tools)
#
# Exit 0 = allow, Exit 2 = block (stderr shown to Claude)

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
STAMP_FILE="$STATE_DIR/engram-searched"  # kept stable: shared with session-start/session-end
LOG_FILE="$LOG_DIR/enforce-kleos-search.log"
mkdir -p "$STATE_DIR" "$LOG_DIR" 2>/dev/null || true

log() {
  echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*" >> "$LOG_FILE" 2>/dev/null || true
}

# Parse tool name and input
TOOL_NAME=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    print(d.get('tool_name', 'unknown'))
except: print('unknown')
" "$INPUT" 2>/dev/null || echo "unknown")

log "Gate check: tool=$TOOL_NAME"

# --- Always-allow tools (read-only / meta) ---
case "$TOOL_NAME" in
  Read|Grep|Glob|Skill|ToolSearch|AskUserQuestion|WebSearch|WebFetch)
    log "Allowed: read-only/meta tool $TOOL_NAME"
    exit 0
    ;;
esac

# --- If stamp exists, everything is allowed ---
if [ -f "$STAMP_FILE" ]; then
  log "Allowed: kleos already searched this session"
  exit 0
fi

# --- For Bash: allow kleos-cli and other bootstrap commands through ---
if [ "$TOOL_NAME" = "Bash" ]; then
  CMD=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    ti = d.get('tool_input', {})
    if isinstance(ti, dict):
        print(ti.get('command', ''))
    else:
        print('')
except: print('')
" "$INPUT" 2>/dev/null || echo "")

  # Allow bootstrap commands (kleos-cli + legacy engram-cli alias)
  if echo "$CMD" | grep -qE '(^|[[:space:]])(kleos-cli|engram-cli|cred|echo|cat|ls|pwd|mkdir|touch|chmod|which|command|test|\[|date|python3|node)([[:space:]]|$)'; then
    # If this is a kleos-cli/engram-cli search, set the stamp
    if echo "$CMD" | grep -qE '(kleos-cli|engram-cli)\s+search'; then
      touch "$STAMP_FILE"
      log "STAMP SET: kleos-cli search detected in command"
    fi
    log "Allowed: bootstrap command"
    exit 0
  fi
fi

# --- For Agent: allow if prompt mentions kleos search ---
if [ "$TOOL_NAME" = "Agent" ]; then
  PROMPT=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    ti = d.get('tool_input', {})
    print(ti.get('prompt', '')[:500])
except: print('')
" "$INPUT" 2>/dev/null || echo "")

  if echo "$PROMPT" | grep -qiE 'kleos|engram'; then
    touch "$STAMP_FILE"
    log "STAMP SET: Agent dispatched for kleos search"
    exit 0
  fi
fi

# --- BLOCKED ---
log "BLOCKED: kleos not searched yet. tool=$TOOL_NAME"
echo "BLOCKED: You have NOT searched Kleos yet this session." >&2
echo "" >&2
echo "You MUST search Kleos before using any action tools." >&2
echo "Run: kleos-cli search \"<relevant query>\" --limit 5" >&2
echo "Or dispatch an Agent to search Kleos." >&2
echo "" >&2
echo "This is non-negotiable. Search Kleos FIRST, then proceed." >&2
exit 2
