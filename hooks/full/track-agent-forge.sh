#!/bin/bash
# PostToolUse hook: tracks when agent-forge tools are called.
# Writes state file that enforce-agent-forge.sh checks.
# Also tracks verify/challenge_code/session_diff for completion gating.
# Always exits 0 (never blocks).

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
LOG_FILE="$LOG_DIR/track-agent-forge.log"
mkdir -p "$STATE_DIR" "$LOG_DIR" 2>/dev/null || true

log() {
  echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*" >> "$LOG_FILE" 2>/dev/null || true
}

TOOL_NAME=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    print(d.get('tool_name', 'unknown'))
except: print('unknown')
" "$INPUT" 2>/dev/null || echo "unknown")

log "PostToolUse: $TOOL_NAME"

FORGE_STATE="$STATE_DIR/agent-forge-active"
FORGE_VERIFY="$STATE_DIR/agent-forge-verified"
FORGE_CHALLENGED="$STATE_DIR/agent-forge-challenged"
FORGE_DIFFED="$STATE_DIR/agent-forge-diffed"

# Extract command if this is a Bash tool call
BASH_CMD=""
if [ "$TOOL_NAME" = "Bash" ]; then
  BASH_CMD=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    ti = d.get('tool_input', {})
    print(ti.get('command', ''))
except: print('')
" "$INPUT" 2>/dev/null || echo "")
fi

# Check for agent-forge CLI calls in Bash commands
if [ -n "$BASH_CMD" ] && echo "$BASH_CMD" | grep -qE 'agent-forge\s+'; then
  if echo "$BASH_CMD" | grep -qE 'agent-forge\s+.*\bspec-task\b'; then
    echo "spec_task" > "$FORGE_STATE"
    rm -f "$FORGE_VERIFY" "$FORGE_CHALLENGED" "$FORGE_DIFFED" 2>/dev/null
    log "State: spec-task active (CLI), completion gates reset"
  elif echo "$BASH_CMD" | grep -qE 'agent-forge\s+.*\blog-hypothesis\b'; then
    echo "log_hypothesis" > "$FORGE_STATE"
    rm -f "$FORGE_VERIFY" "$FORGE_CHALLENGED" "$FORGE_DIFFED" 2>/dev/null
    log "State: log-hypothesis active (CLI), completion gates reset"
  elif echo "$BASH_CMD" | grep -qE 'agent-forge\s+.*\bverify\b'; then
    echo "verified" > "$FORGE_VERIFY"
    log "State: verify completed (CLI)"
  elif echo "$BASH_CMD" | grep -qE 'agent-forge\s+.*\bchallenge-code\b'; then
    echo "challenged" > "$FORGE_CHALLENGED"
    log "State: challenge-code completed (CLI)"
  elif echo "$BASH_CMD" | grep -qE 'agent-forge\s+.*\bsession-diff\b'; then
    echo "diffed" > "$FORGE_DIFFED"
    log "State: session-diff completed (CLI)"
  elif echo "$BASH_CMD" | grep -qE 'agent-forge\s+.*\blog-outcome\b'; then
    rm -f "$FORGE_STATE" "$FORGE_VERIFY" "$FORGE_CHALLENGED" "$FORGE_DIFFED" 2>/dev/null
    log "State: task cycle complete (CLI), all state reset"
  fi
fi

# Legacy MCP tool support (for backwards compatibility)
case "$TOOL_NAME" in
  mcp__agent-forge__spec_task)
    echo "spec_task" > "$FORGE_STATE"
    rm -f "$FORGE_VERIFY" "$FORGE_CHALLENGED" "$FORGE_DIFFED" 2>/dev/null
    log "State: spec_task active (MCP), completion gates reset"
    ;;
  mcp__agent-forge__log_hypothesis)
    echo "log_hypothesis" > "$FORGE_STATE"
    rm -f "$FORGE_VERIFY" "$FORGE_CHALLENGED" "$FORGE_DIFFED" 2>/dev/null
    log "State: log_hypothesis active (MCP), completion gates reset"
    ;;
  mcp__agent-forge__verify)
    echo "verified" > "$FORGE_VERIFY"
    log "State: verify completed (MCP)"
    ;;
  mcp__agent-forge__challenge_code)
    echo "challenged" > "$FORGE_CHALLENGED"
    log "State: challenge_code completed (MCP)"
    ;;
  mcp__agent-forge__session_diff)
    echo "diffed" > "$FORGE_DIFFED"
    log "State: session_diff completed (MCP)"
    ;;
  mcp__agent-forge__log_outcome)
    rm -f "$FORGE_STATE" "$FORGE_VERIFY" "$FORGE_CHALLENGED" "$FORGE_DIFFED" 2>/dev/null
    log "State: task cycle complete (MCP), all state reset"
    ;;
esac

exit 0
