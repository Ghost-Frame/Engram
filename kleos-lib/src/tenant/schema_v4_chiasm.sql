-- Tenant schema v4: chiasm tasks shim.
--
-- Mirrors the `chiasm_tasks` and `chiasm_task_updates` tables from the monolith
-- schema. These two tables were missing from tenant schema_v1, so until this
-- migration ran, `routes/tasks` could only succeed on the monolith fallback.
--
-- Per the TENANT_USERID_SHIM policy: every row in a tenant shard carries the
-- shard owner's user_id. The column stays for now so the kleos-lib services
-- layer does not need changes this phase. Phase 4 will drop all the shim
-- columns workspace-wide.
--
-- Safe to drop-and-recreate: both tables were empty on tenant shards before v4
-- because the routes still went to state.db.

DROP TABLE IF EXISTS chiasm_task_updates;
DROP TABLE IF EXISTS chiasm_tasks;

CREATE TABLE chiasm_tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    project TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK(status IN ('active','paused','blocked','completed')),
    summary TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER NOT NULL DEFAULT 1 -- TENANT_USERID_SHIM
);

CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_status ON chiasm_tasks(status);
CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_agent ON chiasm_tasks(agent);
CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_project ON chiasm_tasks(project);
CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_user ON chiasm_tasks(user_id);

CREATE TABLE chiasm_task_updates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES chiasm_tasks(id) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    status TEXT NOT NULL,
    summary TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER NOT NULL DEFAULT 1 -- TENANT_USERID_SHIM
);

CREATE INDEX IF NOT EXISTS idx_chiasm_task_updates_task_id ON chiasm_task_updates(task_id);
