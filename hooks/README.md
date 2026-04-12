# Engram Hooks for Claude Code

Claude Code hooks that integrate with the Engram memory system.

## Versions

### `simple/` - Minimal Dependencies

Lightweight hooks that only require:
- `engram-cli` in PATH
- `ENGRAM_URL` and `ENGRAM_API_KEY` environment variables
- bash, curl

**Hooks:**
- `session-start.sh` - Loads context from Engram at session start
- `session-end.sh` - Stores session summary when session ends
- `user-prompt.sh` - Searches Engram for relevant memories each turn
- `mnemonic-observe.sh` - Reports tool usage to engram-sidecar

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
- `cred` credential manager (from engram-cred)
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
       "ENGRAM_URL": "http://localhost:4200",
       "ENGRAM_API_KEY": "your-api-key"
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

3. Start engram-server and optionally engram-sidecar

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `ENGRAM_URL` | Yes | Engram server URL (e.g., `http://localhost:4200`) |
| `ENGRAM_API_KEY` | Yes | API key for authentication |
| `ENGRAM_CLI` | No | Path to engram-cli if not in PATH |
| `MNEMONIC_URL` | No | Sidecar URL (default: `http://localhost:7711`) |
| `EIDOLON_URL` | No | Eidolon daemon URL (full hooks only) |
