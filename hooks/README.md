# Kleos Hooks for Claude Code

Claude Code hooks that integrate with the Kleos memory system.

## Versions

### `simple/` - Minimal Dependencies

Lightweight hooks that only require:
- `kleos-cli` in PATH
- `KLEOS_URL` and `KLEOS_API_KEY` environment variables
- bash, curl

**Hooks:**
- `session-start.sh` - Loads context from Kleos at session start
- `session-end.sh` - Stores session summary when session ends
- `user-prompt.sh` - Searches Kleos for relevant memories each turn
- `mnemonic-observe.sh` - Reports tool usage to kleos-sidecar

### `full/` - Full Integration

Advanced hooks with Eidolon guardian integration, cred credential management, and more sophisticated context assembly.

**Additional features:**
- Eidolon brain-aware context generation
- Automatic Mnemonic sidecar startup
- Chiasm task tracking
- Growth/learning materialization
- Agent-Forge protocol enforcement

**Requires:**
- Everything from simple/
- Eidolon daemon (optional, falls back gracefully)
- `cred` credential manager (from kleos-cred)
- python3 (for JSON handling)

## Installation

1. Copy hooks to `~/.claude/hooks/`:
   ```bash
   # Simple version
   cp hooks/simple/*.sh ~/.claude/hooks/

   # Or full version
   cp hooks/full/*.sh ~/.claude/hooks/
   ```

2. Configure in `~/.claude/settings.json`:
   ```json
   {
     "env": {
       "KLEOS_URL": "http://localhost:4200",
       "KLEOS_API_KEY": "your-api-key"
     },
     "hooks": {
       "SessionStart": [{
         "hooks": [{
           "type": "command",
           "command": "bash \"$HOME/.claude/hooks/session-start.sh\"",
           "timeout": 15
         }]
       }],
       "UserPromptSubmit": [{
         "hooks": [{
           "type": "command",
           "command": "bash \"$HOME/.claude/hooks/user-prompt.sh\"",
           "timeout": 8
         }]
       }],
       "Stop": [{
         "hooks": [{
           "type": "command",
           "command": "bash \"$HOME/.claude/hooks/session-end.sh\"",
           "timeout": 15
         }]
       }],
       "PostToolUse": [{
         "matcher": "",
         "hooks": [{
           "type": "command",
           "command": "bash \"$HOME/.claude/hooks/mnemonic-observe.sh\"",
           "timeout": 5
         }]
       }]
     }
   }
   ```

3. Start kleos-server and optionally kleos-sidecar

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `KLEOS_URL` | Yes | Kleos server URL (e.g., `http://localhost:4200`) |
| `KLEOS_API_KEY` | Yes | API key for authentication |
| `KLEOS_CLI` | No | Path to kleos-cli if not in PATH |
| `MNEMONIC_URL` | No | Sidecar URL (default: `http://localhost:7711`) |
| `EIDOLON_URL` | No | Eidolon daemon URL (full hooks only) |
