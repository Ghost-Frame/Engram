-- Tenant schema v7: projects and memory_projects shim.
--
-- Mirrors the monolith `projects` and `memory_projects` tables from
-- kleos-lib/src/db/schema_sql.rs:379. Missing from tenant schema_v1, so
-- routes/projects could only succeed on the monolith fallback until v7.
--
-- Per the TENANT_USERID_SHIM policy: every row in a tenant shard carries the
-- shard owner's user_id. The column stays for now so kleos-lib projects.rs
-- does not need SQL changes this phase. Phase 4 drops it workspace-wide.
--
-- Safe to drop-and-recreate: both tables were empty on tenant shards before
-- v7 because routes still targeted state.db.

DROP TABLE IF EXISTS memory_projects;
DROP TABLE IF EXISTS projects;

CREATE TABLE projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, user_id)
);

CREATE INDEX IF NOT EXISTS idx_projects_user ON projects(user_id);
CREATE INDEX IF NOT EXISTS idx_projects_status ON projects(status);

CREATE TABLE memory_projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(memory_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_memory_projects_memory ON memory_projects(memory_id);
CREATE INDEX IF NOT EXISTS idx_memory_projects_project ON memory_projects(project_id);
