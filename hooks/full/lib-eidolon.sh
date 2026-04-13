#!/bin/bash
# Shared Eidolon helper for all hooks.
# Source this file: . "$HOME/.claude/hooks/lib-eidolon.sh"

_EIDOLON_URL="${EIDOLON_URL:-http://localhost:7700}"
_EIDOLON_KEY=""

# Lazy-resolve Eidolon key (only calls cred once per hook invocation)
eidolon_key() {
  if [ -z "$_EIDOLON_KEY" ]; then
    _EIDOLON_KEY="$(cred get eidolon hetzner --raw 2>/dev/null || echo '')"
  fi
  printf '%s' "$_EIDOLON_KEY"
}

# Call an Eidolon endpoint. Args: METHOD PATH JSON_BODY [TIMEOUT_SECS]
# Prints response body. Returns curl exit code.
eidolon_call() {
  local method="$1" path="$2" body="${3:-}" timeout="${4:-5}"
  local key
  key="$(eidolon_key)"
  if [ -z "$key" ]; then
    echo '{"error":"no eidolon key"}'
    return 1
  fi
  local args=(
    -sf --max-time "$timeout"
    -X "$method"
    -H "Authorization: Bearer $key"
    -H "Content-Type: application/json"
  )
  if [ -n "$body" ]; then
    args+=(-d "$body")
  fi
  curl "${args[@]}" "${_EIDOLON_URL}${path}" 2>/dev/null
}

# JSON-escape a string for embedding in JSON payloads
json_escape() {
  python3 -c "import sys,json; print(json.dumps(sys.argv[1]))" "$1" 2>/dev/null || printf '"%s"' "$1"
}
