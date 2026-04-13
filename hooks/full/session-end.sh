#!/usr/bin/env bash
# session-end.sh - Route session completion through Eidolon daemon.
# Calls /gate/complete (enforces Engram store requirement), /activity (fans out to all services).

set -uo pipefail

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
mkdir -p "$LOG_DIR" 2>/dev/null || true

log() {
  echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] $*" >> "$LOG_DIR/session-end.log" 2>/dev/null || true
}

# Source shared Eidolon helper
. "$HOME_DIR/.claude/hooks/lib-eidolon.sh"

# Read hook event data
INPUT=$(cat 2>/dev/null || echo "{}")
SESSION_ID=$(echo "$INPUT" | python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    print(d.get('session_id', d.get('sessionId', 'unknown')))
except: print('unknown')
" 2>/dev/null || echo "unknown")

log "SessionEnd fired. session_id=$SESSION_ID"

# Notify Mnemonic sidecar to finalize session (fire-and-forget)
curl -sf --max-time 5 "http://localhost:7711/end" -X POST &>/dev/null || true

# Build summary from recent Engram memories (best-effort, local)
ENGRAM_CLI="$HOME_DIR/.local/bin/engram-cli"
SUMMARY=$("$ENGRAM_CLI" recall --limit 3 --json 2>/dev/null | python3 -c "
import sys, json
try:
    memories = json.load(sys.stdin)
    if memories:
        contents = [m.get('content','') for m in memories[:3]]
        joined = ' | '.join(c[:80] for c in contents if c)
        print('Session ended. Recent work: ' + joined if joined else 'Claude Code session ended')
    else:
        print('Claude Code session ended')
except:
    print('Claude Code session ended')
" 2>/dev/null || echo "Claude Code session ended")

# 1. Call Eidolon /gate/complete (checks Engram store requirement)
ESCAPED_SUMMARY=$(json_escape "$SUMMARY")
GATE_RESPONSE=$(eidolon_call POST "/gate/complete" \
  "{\"session_id\":\"$SESSION_ID\",\"summary\":$ESCAPED_SUMMARY}" \
  5 || echo "")

if [ -n "$GATE_RESPONSE" ]; then
  GATE_ALLOWED=$(python3 -c "
import sys, json
try:
    d = json.loads(sys.argv[1])
    print(d.get('allowed', True))
except: print('True')
" "$GATE_RESPONSE" 2>/dev/null || echo "True")
  if [ "$GATE_ALLOWED" = "False" ] || [ "$GATE_ALLOWED" = "false" ]; then
    log "Gate/complete denied -- session may not have stored to Engram"
  fi
fi

# 2. Call Eidolon /activity task.completed (fans out to Chiasm, Axon, Broca, Engram, Brain)
eidolon_call POST "/activity" \
  "{\"agent\":\"claude-code\",\"action\":\"task.completed\",\"summary\":$ESCAPED_SUMMARY,\"project\":\"unknown\"}" \
  5 > /dev/null || true

log "Eidolon activity reported: task.completed"
rm -f /tmp/chiasm-claude-task-id 2>/dev/null || true

# 3. Thymus: auto-evaluate rule compliance for this session
TURN_COUNT_FILE="$STATE_DIR/turn-count-${SESSION_KEY}"
TURN_COUNT=0
if [ -f "$TURN_COUNT_FILE" ]; then
  TURN_COUNT=$(cat "$TURN_COUNT_FILE" 2>/dev/null || echo 0)
fi

ENGRAM_STORES=0
if [ -f "$LOG_DIR/engram-usage.log" ]; then
  ENGRAM_STORES=$(grep -c "store" "$LOG_DIR/engram-usage.log" 2>/dev/null || echo 0)
fi

EIDOLON_CALLS=0
if [ -f "$LOG_DIR/eidolon-activity.log" ]; then
  EIDOLON_CALLS=$(grep -c "activity" "$LOG_DIR/eidolon-activity.log" 2>/dev/null || echo 0)
fi

FORGE_ACTIVE=false
if [ -f "$STATE_DIR/agent-forge-active" ]; then
  FORGE_ACTIVE=true
fi

DRIFT_SIGNALS=0
if [ -f "$LOG_DIR/drift-tracker.log" ]; then
  DRIFT_SIGNALS=$(grep -c "signals=" "$LOG_DIR/drift-tracker.log" 2>/dev/null || echo 0)
fi

# Compute compliance score (0.0-1.0)
SCORE=$(python3 -c "
forge = 1 if '$FORGE_ACTIVE' == 'true' else 0
stores = min(int('$ENGRAM_STORES'), 3)
eidolon = min(int('$EIDOLON_CALLS'), 3)
drift = int('$DRIFT_SIGNALS')
score = (forge * 0.4 + (stores / 3.0) * 0.3 + (eidolon / 3.0) * 0.3) * max(0.5, 1.0 - drift * 0.1)
print(round(max(0.0, min(1.0, score)), 3))
" 2>/dev/null || echo "0.5")

# Route through Eidolon (fans out to Thymus)
EIDOLON_KEY_END="$(cred get eidolon hetzner --raw 2>/dev/null || echo '')"
EIDOLON_URL_END="${EIDOLON_URL:-http://localhost:7700}"
if [ -n "$EIDOLON_KEY_END" ]; then
  curl -sf --max-time 3 "$EIDOLON_URL_END/activity" \
    -X POST \
    -H "Authorization: Bearer $EIDOLON_KEY_END" \
    -H "Content-Type: application/json" \
    -d "{\"agent\":\"claude-code\",\"action\":\"session.quality\",\"summary\":\"Session compliance score: $SCORE (turns=$TURN_COUNT stores=$ENGRAM_STORES eidolon=$EIDOLON_CALLS forge=$FORGE_ACTIVE)\",\"project\":\"drift-mitigation\",\"details\":{\"score\":$SCORE,\"tags\":{\"turns\":\"$TURN_COUNT\",\"engram_stores\":\"$ENGRAM_STORES\",\"eidolon_calls\":\"$EIDOLON_CALLS\",\"forge_active\":\"$FORGE_ACTIVE\",\"drift_signals\":\"$DRIFT_SIGNALS\"}}}" \
    > /dev/null 2>&1 || true
fi

log "Thymus session quality reported via Eidolon: score=$SCORE"

# ── Growth reflection (15% chance, min 10 turns) ──────────────────────
GROWTH_CHANCE=15
GROWTH_MIN_TURNS=10
if [ "${TURN_COUNT:-0}" -ge "$GROWTH_MIN_TURNS" ]; then
  GROWTH_ROLL=$((RANDOM % 100))
  if [ "$GROWTH_ROLL" -lt "$GROWTH_CHANCE" ]; then
    log "Growth reflection triggered (roll=$GROWTH_ROLL, turns=$TURN_COUNT)"

    # Fetch existing growth for anti-repeat
    EXISTING_GROWTH=""
    if [ -f "$HOME_DIR/.claude/GROWTH.md" ]; then
      EXISTING_GROWTH=$(tail -c 4000 "$HOME_DIR/.claude/GROWTH.md" 2>/dev/null || true)
    fi

    # Call Eidolon /growth/reflect
    GROWTH_PAYLOAD=$(cat <<GEOF
{
  "service": "claude-code",
  "context": [
    "Session completed with ${TURN_COUNT:-0} turns",
    "Compliance score: ${SCORE:-unknown}",
    "Engram stores: ${ENGRAM_STORES:-0}",
    "Eidolon reports: ${EIDOLON_CALLS:-0}",
    "Drift signals: ${DRIFT_SIGNALS:-0}",
    "Agent-Forge activated: ${FORGE_ACTIVE:-false}"
  ],
  "existing_growth": $(echo "$EXISTING_GROWTH" | python3 -c 'import sys,json; print(json.dumps(sys.stdin.read()))' 2>/dev/null || echo '""')
}
GEOF
)

    GROWTH_RESULT=$(eidolon_call POST "/growth/reflect" "$GROWTH_PAYLOAD" 2>/dev/null || true)
    if [ -n "$GROWTH_RESULT" ]; then
      OBSERVATION=$(echo "$GROWTH_RESULT" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("observation","") or "")' 2>/dev/null || true)
      if [ -n "$OBSERVATION" ]; then
        log "Growth observation: ${OBSERVATION:0:80}..."
      fi
    fi
  else
    log "Growth reflection skipped (roll=$GROWTH_ROLL >= $GROWTH_CHANCE)"
  fi
fi

# 4. Clean up state files
# Never delete engram-ready-global: it is the cross-session sentinel that
# keeps pre-bash-guardrail from blocking new sessions when PPID-keyed stamps
# haven't been written yet.
if [ "$STAMP_FILE" != "$STATE_DIR/engram-ready-global" ]; then
  rm -f "$STAMP_FILE" 2>/dev/null || true
fi
find "$STATE_DIR" -name 'engram-ready-*' ! -name 'engram-ready-global' -mmin +1440 -delete 2>/dev/null || true
rm -f "$STATE_DIR/agent-forge-active" "$STATE_DIR/agent-forge-verified" \
      "$STATE_DIR/agent-forge-challenged" "$STATE_DIR/agent-forge-diffed" \
      /tmp/claude-forge-bypass 2>/dev/null || true

log "SessionEnd completed"
exit 0
