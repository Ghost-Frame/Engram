-- Tenant schema v74: agent-forge stateful reasoning tables.
--
-- Mirrors the agent-forge local forge.db schema (agent-forge/src/db.rs:32-103)
-- with two changes applied to every table:
--   1. `user_id INTEGER NOT NULL DEFAULT 1` -- row-level tenant isolation in
--      monolith mode where a single DB is shared across users.
--   2. `session_id TEXT` on forge_specs and forge_hypotheses -- required for
--      the gate query that checks whether an active spec covers a given
--      (user, session, file) tuple before allowing a code edit.
--
-- All table names are prefixed `forge_` to avoid collisions with the legacy
-- local agent-forge tables (which used unprefixed names).
--
-- Append-only. No backfill needed: these are new tables.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (74);

CREATE TABLE IF NOT EXISTS forge_specs (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL DEFAULT 1,
    session_id TEXT,
    created_at INTEGER NOT NULL,
    task_description TEXT NOT NULL,
    task_type TEXT NOT NULL,
    acceptance_criteria TEXT NOT NULL,
    interface_contract TEXT,
    edge_cases TEXT,
    files_to_touch TEXT,
    dependencies TEXT,
    status TEXT DEFAULT 'active',
    completed_at INTEGER,
    status_note TEXT
);

CREATE TABLE IF NOT EXISTS forge_hypotheses (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL DEFAULT 1,
    session_id TEXT,
    created_at INTEGER NOT NULL,
    bug_description TEXT NOT NULL,
    hypothesis TEXT NOT NULL,
    confidence REAL NOT NULL,
    outcome TEXT,
    outcome_notes TEXT,
    verified_at INTEGER,
    spec_id TEXT REFERENCES forge_specs(id)
);

CREATE TABLE IF NOT EXISTS forge_approaches (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL DEFAULT 1,
    spec_id TEXT REFERENCES forge_specs(id),
    created_at INTEGER NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    pros TEXT,
    cons TEXT,
    score REAL,
    chosen INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS forge_verifications (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL DEFAULT 1,
    spec_id TEXT REFERENCES forge_specs(id),
    created_at INTEGER NOT NULL,
    command TEXT NOT NULL,
    exit_code INTEGER NOT NULL,
    success INTEGER NOT NULL,
    duration_ms INTEGER,
    criteria_index INTEGER,
    stdout TEXT,
    stderr TEXT
);

CREATE TABLE IF NOT EXISTS forge_checkpoints (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL DEFAULT 1,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    git_ref TEXT,
    files_snapshot TEXT,
    description TEXT
);

CREATE TABLE IF NOT EXISTS forge_session_learns (
    id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL,
    discovery TEXT NOT NULL,
    context TEXT,
    tags TEXT,
    spec_id TEXT REFERENCES forge_specs(id)
);

-- Gate query index: looks up active specs for (user, session) quickly.
CREATE INDEX IF NOT EXISTS idx_forge_specs_gate ON forge_specs(user_id, session_id, status);

-- Chronological listing per user.
CREATE INDEX IF NOT EXISTS idx_forge_specs_user ON forge_specs(user_id, created_at DESC);
