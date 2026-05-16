-- Tenant schema v53: per-agent bearer keys for Chiasm. Mirrors the standalone
-- chiasm `agent_keys` table. Only the SHA-256 hash of the key is stored; the
-- raw key is returned exactly once at creation.

CREATE TABLE IF NOT EXISTS chiasm_agent_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    key_prefix TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_used_at TEXT,
    revoked INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_chiasm_agent_keys_hash ON chiasm_agent_keys(key_hash);
CREATE INDEX IF NOT EXISTS idx_chiasm_agent_keys_agent ON chiasm_agent_keys(agent);
