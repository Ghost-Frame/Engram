-- Tenant schema v52: Syntheos parity -- extend Chiasm with dependencies,
-- path claims, and extended task fields to match standalone TypeScript.

-- Task dependencies (DAG edges for blocking relationships)
CREATE TABLE IF NOT EXISTS chiasm_task_dependencies (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES chiasm_tasks(id) ON DELETE CASCADE,
    depends_on INTEGER NOT NULL REFERENCES chiasm_tasks(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(task_id, depends_on)
);

CREATE INDEX IF NOT EXISTS idx_chiasm_deps_task ON chiasm_task_dependencies(task_id);
CREATE INDEX IF NOT EXISTS idx_chiasm_deps_depends ON chiasm_task_dependencies(depends_on);

-- Path claims (resource locking during multi-agent execution)
CREATE TABLE IF NOT EXISTS chiasm_path_claims (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES chiasm_tasks(id) ON DELETE CASCADE,
    agent TEXT NOT NULL,
    project TEXT NOT NULL,
    path TEXT NOT NULL,
    claimed_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT NOT NULL,
    released INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_chiasm_claims_project_path ON chiasm_path_claims(project, path);
CREATE INDEX IF NOT EXISTS idx_chiasm_claims_task ON chiasm_path_claims(task_id);
CREATE INDEX IF NOT EXISTS idx_chiasm_claims_expires ON chiasm_path_claims(expires_at);
