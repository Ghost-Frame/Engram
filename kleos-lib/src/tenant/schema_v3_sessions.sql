-- v3: align tenant sessions + session_output schema with monolith (user_id shim).
--
-- Phase 2 shimmed user_id on memories/artifacts/vector_sync_pending. v2 caught
-- scratchpad. v3 catches sessions: the tenant v1 sessions table is missing
-- user_id entirely (kleos-lib sessions.rs INSERTs and WHEREs it), and the
-- tenant v1 schema has no session_output table at all (kleos-lib appends and
-- reads from it). Without this shim, the ResolvedDb route swap in Phase 3.2
-- would hit "no such column: user_id" and "no such table: session_output" on
-- tenant shards.
--
-- This migration drops and recreates both tables to match the monolith shape.
-- Tenant sessions / session_output tables have zero rows at v1/v2 because the
-- routes still hit state.db (monolith), so the drop is safe.
--
-- Drop order is child-first (session_output references sessions). Create order
-- is parent-first. Grep TENANT_USERID_SHIM to find every shim touchpoint;
-- these columns become shims until Phase 4 reverses them workspace-wide.

DROP TABLE IF EXISTS session_output;
DROP TABLE IF EXISTS sessions;

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    user_id INTEGER NOT NULL DEFAULT 0, -- TENANT_USERID_SHIM
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE session_output (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    line TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_sessions_user ON sessions(user_id); -- TENANT_USERID_SHIM
CREATE INDEX idx_session_output_session ON session_output(session_id);
