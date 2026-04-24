-- Tenant schema v23: drop user_id from scratchpad (first UNIQUE-rebuild).
--
-- v2 introduced scratchpad as a user_id shim table with
-- UNIQUE(user_id, session, entry_key). SQLite's ALTER TABLE DROP COLUMN
-- cannot remove a column that participates in a UNIQUE constraint, so we
-- follow the 12-step table-rebuild pattern.
--
-- The new constraint is UNIQUE(session, agent, entry_key). Widening from
-- (user_id, session, entry_key) is safe: all surviving rows were written
-- under user_id=1 so the old (user_id, session, entry_key) collapsed to
-- (session, entry_key); the new constraint permits a strict superset so
-- no existing row can violate it.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (23);

PRAGMA foreign_keys = OFF;

-- 1. rename out of the way
ALTER TABLE scratchpad RENAME TO _scratchpad_old_v22;

-- 2. drop shim indexes that reference the old table name / user_id column
DROP INDEX IF EXISTS idx_scratchpad_agent;
DROP INDEX IF EXISTS idx_scratchpad_expires;
DROP INDEX IF EXISTS idx_scratchpad_user_expires;
DROP INDEX IF EXISTS idx_scratchpad_session;

-- 3. create the new table without user_id
CREATE TABLE scratchpad (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL DEFAULT 'unknown',
    session TEXT NOT NULL DEFAULT 'default',
    model TEXT NOT NULL DEFAULT '',
    entry_key TEXT NOT NULL,
    value TEXT NOT NULL DEFAULT '',
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(session, agent, entry_key)
);

-- 4. copy rows forward, dropping the user_id column
INSERT INTO scratchpad (id, agent, session, model, entry_key, value, expires_at, created_at, updated_at)
SELECT id, agent, session, model, entry_key, value, expires_at, created_at, updated_at
FROM _scratchpad_old_v22;

-- 5. drop the old table
DROP TABLE _scratchpad_old_v22;

-- 6. recreate supporting indexes without user_id
CREATE INDEX idx_scratchpad_agent ON scratchpad(agent);
CREATE INDEX idx_scratchpad_session ON scratchpad(session);
CREATE INDEX idx_scratchpad_expires ON scratchpad(expires_at) WHERE expires_at IS NOT NULL;

PRAGMA foreign_keys = ON;
