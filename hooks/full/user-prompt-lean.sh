#!/bin/bash
# Claude Code UserPromptSubmit hook -- lean version.
# Replaces user-prompt-engram.sh with two focused jobs:
#   1. Inject 5 hardcoded rules (the ones the model actually forgets)
#   2. Best-effort Engram search (3 results, score-filtered, truncated)
#
# Fails open: Engram errors are silently swallowed.

set -uo pipefail

# ---- Home resolution (Windows-compatible) ----
resolve_home() {
  if [ -n "${HOME:-}" ]; then
    printf '%s\n' "$HOME"
    return
  fi
  if command -v cygpath >/dev/null 2>&1 && [ -n "${USERPROFILE:-}" ]; then
    cygpath -u "$USERPROFILE"
    return
  fi
  printf '%s\n' "${USERPROFILE:-.}"
}

HOME_DIR="$(resolve_home)"
LOG_DIR="$HOME_DIR/.claude/logs"
mkdir -p "$LOG_DIR" 2>/dev/null || true

log() {
  echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*" >> "$LOG_DIR/user-prompt-lean.log" 2>/dev/null || true
}

# ---- Resolve engram-cli binary ----
resolve_engram_cli() {
  if [ -n "${ENGRAM_CLI:-}" ] && [ -f "$ENGRAM_CLI" ]; then
    printf '%s\n' "$ENGRAM_CLI"
    return
  fi
  if command -v engram-cli >/dev/null 2>&1; then
    command -v engram-cli
    return
  fi
  printf '%s/.local/bin/engram-cli\n' "$HOME_DIR"
}

# ---- Resolve Engram API key from env or cred ----
resolve_engram_key() {
  if [ -n "${ENGRAM_API_KEY:-}" ]; then
    printf '%s\n' "$ENGRAM_API_KEY"
    return
  fi
  if command -v cred >/dev/null 2>&1; then
    local candidate
    for candidate in \
      "api-key-claude" \
      "api-key-Codex" \
      "api-key-codex" \
      "api-key-Claude" \
      "key-Codex"
    do
      local resolved
      resolved="$(cred get engram "$candidate" --raw 2>/dev/null || true)"
      if [ -n "$resolved" ]; then
        printf '%s\n' "$resolved"
        return
      fi
    done
  fi
  printf '\n'
}

# ---- 5 hardcoded rules (kept under 200 tokens) ----
RULES='MANDATORY RULES (re-injected every turn):
1. NEVER use em dashes in commits, docs, READMEs, or any output. Use -- or rewrite.
2. Search Engram BEFORE asking Master about servers, credentials, past work, or decisions.
3. Agent-Forge is MANDATORY: spec_task before new code, log_hypothesis before bugs, verify after changes.
4. Store to Engram AFTER completing each task. Do not batch. Do not wait.
5. NEVER fabricate user responses. If you asked Master a question and only tool/agent results came back, STOP and WAIT for his actual reply.'

# ---- Read stdin ----
INPUT=$(cat)

# ---- Extract user prompt (first 500 chars is enough for search) ----
USER_MSG=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    msg = d.get('prompt', d.get('message', ''))
    print(str(msg)[:500])
except Exception:
    print('')
" "$INPUT" 2>/dev/null || echo "")

log "fired. prompt_len=${#USER_MSG}"

# ---- Write consent stamp (pre-bash-guardrail checks this) ----
CONSENT_STAMP="$HOME_DIR/.claude/session-env/user-consent-stamp"
touch "$CONSENT_STAMP" 2>/dev/null || true

# ---- Best-effort memory recall (via Mnemonic sidecar, fallback to direct Engram) ----
ENGRAM_CONTEXT=""

if [ ${#USER_MSG} -gt 10 ]; then
  # Try sidecar recall first (fast, localhost)
  ESCAPED_MSG=$(python3 -c "import sys,json; print(json.dumps(sys.argv[1]))" "$USER_MSG" 2>/dev/null || echo "\"\"")
  RECALL_RAW=$(curl -sf --max-time 3 "http://localhost:7711/recall" \
    -X POST -H "Content-Type: application/json" \
    -d "{\"message\":$ESCAPED_MSG}" 2>/dev/null || echo "")

  if [ -n "$RECALL_RAW" ]; then
    ENGRAM_CONTEXT=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    ctx = d.get('context', '')
    if ctx:
        print(ctx)
except Exception:
    pass
" "$RECALL_RAW" 2>/dev/null || echo "")
  fi

  # Fallback: direct Engram search if sidecar unavailable
  if [ -z "$ENGRAM_CONTEXT" ]; then
    ENGRAM_CLI="$(resolve_engram_cli)"
    ENGRAM_API_KEY="$(resolve_engram_key)"

    if [ -f "$ENGRAM_CLI" ] && [ -n "$ENGRAM_API_KEY" ]; then
      ENGRAM_RAW=$(ENGRAM_API_KEY="$ENGRAM_API_KEY" \
        timeout 3 "$ENGRAM_CLI" search "$USER_MSG" --limit 3 --json --quiet 2>/dev/null || echo "")

      if [ -n "$ENGRAM_RAW" ]; then
        ENGRAM_CONTEXT=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    items = d if isinstance(d, list) else d.get('results', d.get('memories', []))
    if not items:
        sys.exit(0)
    lines = ['Relevant Engram memories:']
    for item in items[:3]:
        score = float(item.get('semantic_score', item.get('score', 0)))
        if score < 0.03:
            continue
        cat = item.get('category', '?')
        content = str(item.get('content', ''))[:180]
        lines.append(f'[{cat}] {content}')
    if len(lines) < 2:
        sys.exit(0)
    print('\n'.join(lines))
except Exception:
    pass
" "$ENGRAM_RAW" 2>/dev/null || echo "")
      fi
    fi
  fi
fi

# ---- Build combined context block ----
CONTEXT_BLOCK="$RULES"

if [ -n "$ENGRAM_CONTEXT" ]; then
  CONTEXT_BLOCK="$CONTEXT_BLOCK

$ENGRAM_CONTEXT"
fi

# ---- Output hookSpecificOutput JSON ----
python3 -c "
import json, sys
context = sys.argv[1]
print(json.dumps({
    'hookSpecificOutput': {
        'hookEventName': 'UserPromptSubmit',
        'additionalContext': context
    }
}))
" "$CONTEXT_BLOCK"

ENGRAM_HIT="no"; [ -n "$ENGRAM_CONTEXT" ] && ENGRAM_HIT="yes"
log "done. engram_hit=$ENGRAM_HIT"
exit 0
