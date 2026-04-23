-- Tenant schema v13: loom workflow tables.
--
-- Mirrors the monolith shapes at kleos-lib/src/db/schema_sql.rs:984-1046:
--   * loom_workflows (user_id, UNIQUE(user_id, name))
--   * loom_runs (workflow_id FK, user_id)
--   * loom_steps (run_id FK, no user_id -- scoped via run)
--   * loom_run_logs (run_id FK, step_id FK, no user_id -- scoped via run)
--
-- Per TENANT_USERID_SHIM: user_id stays on tables that carry it in the
-- monolith, with DEFAULT 1. Phase 4 will strip these columns.
--
-- Safe to drop-and-recreate: these tables were empty on tenant shards
-- before v13 because routes/loom still targeted state.db.

DROP TABLE IF EXISTS loom_run_logs;
DROP TABLE IF EXISTS loom_steps;
DROP TABLE IF EXISTS loom_runs;
DROP TABLE IF EXISTS loom_workflows;

CREATE TABLE loom_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    steps TEXT NOT NULL DEFAULT '[]',
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, name)
);

CREATE INDEX IF NOT EXISTS idx_loom_workflows_user ON loom_workflows(user_id);

CREATE TABLE loom_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id INTEGER NOT NULL REFERENCES loom_workflows(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    input TEXT NOT NULL DEFAULT '{}',
    output TEXT NOT NULL DEFAULT '{}',
    error TEXT,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    started_at TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_loom_runs_workflow ON loom_runs(workflow_id);
CREATE INDEX IF NOT EXISTS idx_loom_runs_status ON loom_runs(status);
CREATE INDEX IF NOT EXISTS idx_loom_runs_user ON loom_runs(user_id);

CREATE TABLE loom_steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id INTEGER NOT NULL REFERENCES loom_runs(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'pending',
    input TEXT NOT NULL DEFAULT '{}',
    output TEXT NOT NULL DEFAULT '{}',
    error TEXT,
    depends_on TEXT NOT NULL DEFAULT '[]',
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 3,
    timeout_ms INTEGER NOT NULL DEFAULT 30000,
    started_at TEXT,
    completed_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_loom_steps_run ON loom_steps(run_id);
CREATE INDEX IF NOT EXISTS idx_loom_steps_status ON loom_steps(status);

CREATE TABLE loom_run_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id INTEGER NOT NULL REFERENCES loom_runs(id) ON DELETE CASCADE,
    step_id INTEGER REFERENCES loom_steps(id) ON DELETE SET NULL,
    level TEXT NOT NULL DEFAULT 'info',
    message TEXT NOT NULL,
    data TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_loom_run_logs_run ON loom_run_logs(run_id);
