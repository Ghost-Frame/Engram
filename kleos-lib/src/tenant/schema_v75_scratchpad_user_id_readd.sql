-- Tenant schema v75: re-add user_id to scratchpad (reverses v23).
--
-- v23 dropped user_id from scratchpad on the per-shard-file isolation
-- assumption, widening UNIQUE(user_id, session, entry_key) to
-- UNIQUE(session, agent, entry_key) with the note "all surviving rows were
-- written under user_id=1". That assumption holds only until a second tenant
-- writes: in single-DB (monolith) mode every tenant shares one scratchpad
-- table, so an unscoped read (scratchpad::list_entries) returns other users'
-- entries -- a cross-tenant working-memory leak in assemble_context -- and an
-- unscoped delete or ON CONFLICT upsert can clobber another tenant's row.
--
-- This migration restores user_id as a universal, always-applied predicate so
-- scratchpad read/enumerate/delete/upsert are correct in every deployment mode.
-- In a shard the predicate is a no-op once existing rows are backfilled to the
-- shard owner's user_id -- the runner (run_tenant_migrations) performs that
-- backfill after this file runs (see TENANT_MIGRATION_READD_USER_ID_SCRATCHPAD
-- / backfill_owner_tables_for_version).
--
-- scratchpad participates in UNIQUE(session, agent, entry_key), and SQLite
-- cannot alter a column set that participates in a UNIQUE constraint in place,
-- so we follow the same 12-step table-rebuild pattern v23 used (in reverse):
-- rebuild the table WITH user_id and the tightened
-- UNIQUE(user_id, session, agent, entry_key). Tightening the constraint is safe
-- here: every surviving row carries the column default user_id=1 until the
-- runner backfills it, and no two rows can collide on the wider key that did not
-- already collide on the old (session, agent, entry_key) key.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (75);

PRAGMA foreign_keys = OFF;

-- 1. rename the v23 table out of the way
ALTER TABLE scratchpad RENAME TO _scratchpad_old_v74;

-- 2. drop the v23 indexes that reference the old table name
DROP INDEX IF EXISTS idx_scratchpad_agent;
DROP INDEX IF EXISTS idx_scratchpad_session;
DROP INDEX IF EXISTS idx_scratchpad_expires;

-- 3. create the new table WITH user_id and the user-scoped UNIQUE constraint
CREATE TABLE scratchpad (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
    agent TEXT NOT NULL DEFAULT 'unknown',
    session TEXT NOT NULL DEFAULT 'default',
    model TEXT NOT NULL DEFAULT '',
    entry_key TEXT NOT NULL,
    value TEXT NOT NULL DEFAULT '',
    expires_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, session, agent, entry_key)
);

-- 4. copy rows forward; user_id takes the column default (1) and is then
--    backfilled to the shard owner by the runner immediately after this file
INSERT INTO scratchpad (id, agent, session, model, entry_key, value, expires_at, created_at, updated_at)
SELECT id, agent, session, model, entry_key, value, expires_at, created_at, updated_at
FROM _scratchpad_old_v74;

-- 5. drop the old table
DROP TABLE _scratchpad_old_v74;

-- 6. recreate supporting indexes, now including a user-scoped lookup index
CREATE INDEX idx_scratchpad_agent ON scratchpad(agent);
CREATE INDEX idx_scratchpad_session ON scratchpad(session);
CREATE INDEX idx_scratchpad_expires ON scratchpad(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX idx_scratchpad_user ON scratchpad(user_id);

PRAGMA foreign_keys = ON;
