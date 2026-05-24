-- Tenant schema v65: re-add user_id to the five intelligence tables that
-- v38 dropped and v62 did not restore: current_state, reconsolidations,
-- temporal_patterns, digests, memory_feedback.
--
-- current_state carried UNIQUE(agent, key) before v38.  Proper per-user
-- isolation requires UNIQUE(agent, key, user_id), so a 12-step table rebuild
-- is needed (same pattern as the entities rebuild in v63).
--
-- The other four tables take a simple ALTER TABLE ADD COLUMN path.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (65);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;

ALTER TABLE current_state RENAME TO _current_state_old_v65;
DROP INDEX IF EXISTS idx_current_state_agent;
DROP INDEX IF EXISTS idx_current_state_user;
DROP INDEX IF EXISTS idx_cs_key;
DROP INDEX IF EXISTS idx_cs_key_user;

CREATE TABLE current_state (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
    previous_value TEXT,
    previous_memory_id INTEGER,
    updated_count INTEGER NOT NULL DEFAULT 1,
    user_id INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(agent, key, user_id)
);

INSERT INTO current_state
    (id, agent, key, value, memory_id, previous_value, previous_memory_id,
     updated_count, user_id, updated_at, created_at)
SELECT
    id, agent, key, value, memory_id, previous_value, previous_memory_id,
    updated_count, 1, updated_at, created_at
FROM _current_state_old_v65;

DROP TABLE _current_state_old_v65;

CREATE INDEX IF NOT EXISTS idx_current_state_agent ON current_state(agent);
CREATE INDEX IF NOT EXISTS idx_current_state_user ON current_state(user_id);
CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_cs_key_user ON current_state(key, user_id);

PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;

-- Simple ADD COLUMN for the remaining four tables. These are NOT individually
-- pragma-guarded; idempotency relies on the schema_migrations version gate in
-- the tenant migration runner (this file only executes if version < 65).
ALTER TABLE reconsolidations ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE temporal_patterns ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE digests ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE memory_feedback ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_reconsolidations_user ON reconsolidations(user_id);
CREATE INDEX IF NOT EXISTS idx_temporal_patterns_user ON temporal_patterns(user_id);
CREATE INDEX IF NOT EXISTS idx_digests_user ON digests(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_user ON memory_feedback(user_id);
