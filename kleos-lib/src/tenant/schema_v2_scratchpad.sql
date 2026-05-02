-- v2: align tenant scratchpad schema with monolith (user_id shim).
--
-- Phase 2 shimmed user_id on memories/artifacts/vector_sync_pending but
-- missed scratchpad. kleos-lib scratchpad queries still carry a user_id
-- filter (until Phase 4 drops user_id workspace-wide), and their INSERTs
-- use ON CONFLICT(user_id, session, entry_key). Without the shim, the
-- ResolvedDb route swap in Phase 3.1 would hit "no such column: user_id"
-- on tenant shards.
--
-- This migration drops and recreates scratchpad to match the monolith
-- shape. Tenant scratchpad tables have no rows at v1 -- the routes
-- still used state.db -- so the drop is safe.
--
-- Grep TENANT_USERID_SHIM to find every shim touchpoint; this whole
-- table becomes a shim until Phase 4 reverses it.

DROP TABLE IF EXISTS scratchpad;

CREATE TABLE scratchpad (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL DEFAULT 'unknown',
    session TEXT NOT NULL DEFAULT 'default',
    model TEXT NOT NULL DEFAULT '',
    entry_key TEXT NOT NULL,
    value TEXT NOT NULL DEFAULT '',
    expires_at TEXT,
    user_id INTEGER NOT NULL DEFAULT 0, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, session, entry_key)
);

CREATE INDEX idx_scratchpad_agent ON scratchpad(agent);
CREATE INDEX idx_scratchpad_expires ON scratchpad(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX idx_scratchpad_user_expires ON scratchpad(user_id, expires_at); -- TENANT_USERID_SHIM
CREATE INDEX idx_scratchpad_session ON scratchpad(user_id, session); -- TENANT_USERID_SHIM
