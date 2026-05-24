-- Tenant schema v58: re-add user_id to soma_agents with UNIQUE(name, user_id).
--
-- v8 created soma_agents with user_id + UNIQUE(name); v29 dropped user_id under
-- the per-shard-only isolation assumption. The single-DB-mode repair restores
-- user_id, but the table's UNIQUE(name) must become UNIQUE(name, user_id) so
-- distinct users can own identically-named agents and the register upsert
-- cannot clobber another user's row. Changing a UNIQUE constraint requires the
-- 12-step rebuild (mirrors monolith migration 67).
--
-- soma_agents is FK-referenced by soma_agent_groups and soma_agent_logs
-- (ON DELETE CASCADE); the rebuild preserves id values and runs with
-- PRAGMA foreign_keys = OFF so those references stay valid. The runner
-- backfills the copied DEFAULT-1 rows to the shard owner after this file runs.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (58);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

ALTER TABLE soma_agents RENAME TO _soma_agents_old_v58;

DROP INDEX IF EXISTS idx_soma_agents_type;
DROP INDEX IF EXISTS idx_soma_agents_status;

CREATE TABLE soma_agents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    description TEXT,
    capabilities TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending','online','offline','error')),
    config TEXT NOT NULL DEFAULT '{}',
    heartbeat_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    quality_score REAL,
    drift_flags TEXT DEFAULT '[]',
    user_id INTEGER NOT NULL DEFAULT 1,
    UNIQUE(name, user_id)
);

INSERT INTO soma_agents
    (id, name, type, description, capabilities, status, config, heartbeat_at,
     created_at, updated_at, quality_score, drift_flags, user_id)
SELECT id, name, type, description, capabilities, status, config, heartbeat_at,
       created_at, updated_at, quality_score, drift_flags, 1
FROM _soma_agents_old_v58;

DROP TABLE _soma_agents_old_v58;

CREATE INDEX idx_soma_agents_type ON soma_agents(type);
CREATE INDEX idx_soma_agents_status ON soma_agents(status);
CREATE INDEX idx_soma_agents_user ON soma_agents(user_id);

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
