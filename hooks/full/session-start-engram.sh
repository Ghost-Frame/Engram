#!/bin/bash
# Claude Code SessionStart hook: bootstrap context via Eidolon daemon.
# Calls Eidolon /prompt/generate for brain-aware context, /activity for registration.
# Falls back to direct engram-cli if Eidolon unreachable.

set -euo pipefail

# Skip for subagents -- only main CLI sessions get full context bootstrap.
if [ -n "${CLAUDE_CODE_ENTRYPOINT:-}" ] && [ "$CLAUDE_CODE_ENTRYPOINT" != "cli" ]; then
  exit 0
fi

resolve_home() {
  if [ -n "${HOME:-}" ]; then printf '%s\n' "$HOME"; return; fi
  if command -v cygpath >/dev/null 2>&1 && [ -n "${USERPROFILE:-}" ]; then
    cygpath -u "$USERPROFILE"; return
  fi
  printf '%s\n' "${USERPROFILE:-.}"
}

HOME_DIR="$(resolve_home)"
LOG_DIR="$HOME_DIR/.claude/logs"
STATE_DIR="$HOME_DIR/.claude/session-env"
SESSION_KEY="${PPID:-$$}"
STAMP_FILE="$STATE_DIR/engram-ready-${SESSION_KEY}"
mkdir -p "$LOG_DIR" "$STATE_DIR" 2>/dev/null || true

# --- Write bootstrap stamps IMMEDIATELY (before any fallible call). ---
# Rationale: the rest of this script makes network calls to Eidolon and
# engram-cli under `set -euo pipefail`. Any sporadic failure there used to
# silently kill the script before the stamp was written, leaving the
# pre-bash-guardrail blocking every subsequent command. The stamp represents
# "bootstrap started" not "bootstrap succeeded" -- the guardrail fallback
# just needs any engram-ready-* file to exist. We rewrite STAMP_FILE with
# the success marker at the end of the script.
_NOW="$(date -u '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null || echo unknown)"
printf 'bootstrapping\t%s\t%s\n' "$_NOW" "early" > "$STATE_DIR/engram-ready-${PPID:-$$}" 2>/dev/null || true
printf 'global\t%s\n' "$_NOW" > "$STATE_DIR/engram-ready-global" 2>/dev/null || true

log() {
  echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*" >> "$LOG_DIR/session-start.log" 2>/dev/null || true
}

log "Wrote early bootstrap stamps (PPID=${PPID:-$$})"

# Source shared Eidolon helper
. "$HOME_DIR/.claude/hooks/lib-eidolon.sh"

# --- Clear engram-search enforcement stamp from previous session ---
rm -f "$STATE_DIR/engram-searched" 2>/dev/null || true
log "Cleared engram-searched stamp"

log "SessionStart fired. HOME_DIR=$HOME_DIR"

# --- 0. Start Mnemonic sidecar if not running ---
if ! curl -sf --max-time 1 "http://localhost:7711/health" > /dev/null 2>&1; then
  MNEMONIC_BIN="$HOME_DIR/.local/lib/mnemonic/index.ts"
  if [ -f "$MNEMONIC_BIN" ]; then
    ENGRAM_URL="${ENGRAM_URL:-http://localhost:4200}" ENGRAM_API_KEY="${ENGRAM_API_KEY:-}" nohup node --experimental-strip-types "$MNEMONIC_BIN" >> "$LOG_DIR/mnemonic.log" 2>&1 &
    disown
    log "Started Mnemonic sidecar (pid=$!)"
    # Give it a moment to bind
    sleep 0.5
  else
    log "Mnemonic binary not found at $MNEMONIC_BIN"
  fi
fi

# --- 1. Call Eidolon /prompt/generate for brain-aware context ---
PROMPT_RESULT=""
PROMPT_RESPONSE=$(eidolon_call POST "/prompt/generate" \
  '{"task":"session-bootstrap agent-rules infrastructure active-tasks recent-decisions","agent":"claude-code"}' \
  8 || echo "")

if [ -n "$PROMPT_RESPONSE" ]; then
  PROMPT_RESULT=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    prompt = d.get('prompt', '')
    if prompt:
        print(prompt)
except:
    pass
" "$PROMPT_RESPONSE" 2>/dev/null || echo "")
fi

log "Eidolon /prompt/generate returned ${#PROMPT_RESULT} chars"

# --- 2. Register session via Eidolon /activity (fans out to Chiasm, Axon, Broca, Brain) ---
ACTIVITY_RESPONSE=$(eidolon_call POST "/activity" \
  '{"agent":"claude-code","action":"task.started","summary":"Claude Code session started","project":"unknown"}' \
  5 || echo "")

# Extract Chiasm task ID from fanout response
if [ -n "$ACTIVITY_RESPONSE" ]; then
  TASK_ID=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    chiasm = d.get('fanout', {}).get('chiasm', {})
    tid = chiasm.get('created', chiasm.get('id', chiasm.get('auto_created', '')))
    if tid: print(str(tid))
except: pass
" "$ACTIVITY_RESPONSE" 2>/dev/null || echo "")
  if [ -n "$TASK_ID" ]; then
    echo "$TASK_ID" > /tmp/chiasm-claude-task-id
    log "Chiasm task via Eidolon: $TASK_ID"
  fi
fi

# --- 3. Recent memories via engram-cli (lightweight, local) ---
resolve_engram_cli() {
  if [ -n "${ENGRAM_CLI:-}" ]; then printf '%s\n' "$ENGRAM_CLI"; return; fi
  if command -v engram-cli >/dev/null 2>&1; then command -v engram-cli; return; fi
  printf '%s/.local/bin/engram-cli\n' "$HOME_DIR"
}

ENGRAM_CLI="$(resolve_engram_cli)"
RECENT_MEMORIES=""
if [ -f "$ENGRAM_CLI" ]; then
  LIST_RAW=$("$ENGRAM_CLI" list --limit 5 --json --quiet 2>/dev/null || echo "")
  if [ -n "$LIST_RAW" ]; then
    RECENT_MEMORIES=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    items = d if isinstance(d, list) else d.get('memories', d.get('results', []))
    lines = []
    for item in items:
        cat = item.get('category', 'unknown')
        content = str(item.get('content', ''))[:200]
        lines.append(f'[{cat}] {content}')
    print('\n'.join(lines))
except: pass
" "$LIST_RAW" 2>/dev/null || echo "")
  fi
fi

# --- 4. Fallback: if Eidolon unreachable, query engram-cli directly ---
if [ -z "$PROMPT_RESULT" ]; then
  log "Eidolon unreachable, falling back to direct engram-cli"
  _resolve_engram_key() {
    if [ -n "${ENGRAM_API_KEY:-}" ]; then printf '%s\n' "$ENGRAM_API_KEY"; return; fi
    if command -v cred >/dev/null 2>&1; then
      local resolved
      resolved="$(cred get engram api-key-claude --raw 2>/dev/null || true)"
      if [ -n "$resolved" ]; then printf '%s\n' "$resolved"; return; fi
    fi
    printf '\n'
  }
  ENGRAM_API_KEY="$(_resolve_engram_key)"
  if [ -f "$ENGRAM_CLI" ] && [ -n "$ENGRAM_API_KEY" ]; then
    PROMPT_RESULT=$("$ENGRAM_CLI" context "agent-rules critical infrastructure active-tasks recent-decisions personality" --budget 3000 --quiet 2>/dev/null || echo "")
  fi
fi

# --- Handle total failure ---
if [ -z "$PROMPT_RESULT" ] && [ -z "$RECENT_MEMORIES" ]; then
  log "No context from Eidolon or Engram"
  echo "EIDOLON AND ENGRAM UNREACHABLE. Do NOT proceed with infrastructure work until context is confirmed."
  echo "MANDATORY: Use engram-cli to check connectivity."
  exit 0
fi

# --- Build context block ---
CONTEXT_BLOCK=""

if [ -n "$PROMPT_RESULT" ]; then
  CONTEXT_BLOCK+="=== EIDOLON LIVING CONTEXT ===
$PROMPT_RESULT"
fi

if [ -n "$RECENT_MEMORIES" ]; then
  CONTEXT_BLOCK+="

=== RECENT MEMORIES ===
$RECENT_MEMORIES"
fi

CONTEXT_BLOCK+="

=== MANDATORY RULES ===
Use the Engram skill and local engram-cli via Git Bash (Bash tool) for ALL Engram operations (search, store, list, context). Never use curl for Engram. Do NOT use WSL.
Use OpenSpace MCP tools for all OpenSpace operations.
Search Engram BEFORE asking Master any question about servers, credentials, or past decisions.
Store outcomes to Engram AFTER completing any task. Do not batch. Do not wait.
If Chiasm task ID exists at /tmp/chiasm-claude-task-id, update task status on changes.
EIDOLON: Before ANY destructive/irreversible action, the pre-bash-guardrail hook handles gate checks automatically. If gate returns deny, STOP and ask Master. No exceptions."

# ── Growth materialization ─────────────────────────────────────────────
GROWTH_MD=""
GROWTH_RESULT=$(eidolon_call GET "/growth/materialize?service=claude-code&limit=30&max_bytes=16000" 2>/dev/null || true)
if [ -n "$GROWTH_RESULT" ] && [ "$GROWTH_RESULT" != "null" ]; then
  # /growth/materialize returns plain text, not JSON
  echo "$GROWTH_RESULT" > "$HOME_DIR/.claude/GROWTH.md"
  GROWTH_MD="$GROWTH_RESULT"
  log "Materialized GROWTH.md ($(echo "$GROWTH_RESULT" | wc -c) bytes)"
fi

# Write stamp
printf 'ready\t%s\t%s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "eidolon" > "$STAMP_FILE"
log "Wrote session bootstrap stamp"

# Output structured JSON
python3 -c "
import json, sys
context = sys.argv[1]
growth = sys.argv[2] if len(sys.argv) > 2 else ''
if growth:
    context = context + '\n\n## Growth & Learnings\n' + growth
output = {
    'hookSpecificOutput': {
        'hookEventName': 'SessionStart',
        'additionalContext': context
    }
}
print(json.dumps(output))
" "$CONTEXT_BLOCK" "$GROWTH_MD"

log "SessionStart completed"
exit 0
