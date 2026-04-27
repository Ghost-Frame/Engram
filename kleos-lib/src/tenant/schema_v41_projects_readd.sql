-- Tenant schema v41: re-add user_id to projects (C-R3-004).
--
-- v28 dropped user_id from projects under the assumption that every shard
-- holds exactly one tenant's data, making the column redundant. The R-3
-- audit (C-R3-004) showed the helper SQL still benefits from carrying the
-- column so the same query works on monolith and on shard. Defense in
-- depth: a shard accidentally crossed (e.g. by a future bulk import that
-- writes to the wrong DB) would be caught by the user_id filter.
--
-- The shard owner's user_id is unknown at migration time, so legacy rows
-- backfill to user_id=1. New inserts must pass the real user_id (the
-- helper does this -- see kleos-lib/src/projects.rs).
--
-- Idempotent: each migration runs once per shard via schema_migrations.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (41);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- 1. rename out of the way
ALTER TABLE projects RENAME TO _projects_old_v40;

-- 2. drop indexes that reference the old shape
DROP INDEX IF EXISTS idx_projects_status;

-- 3. recreate with user_id + UNIQUE(name, user_id)
CREATE TABLE projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    metadata TEXT,
    user_id INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, user_id)
);

-- 4. copy rows forward, defaulting user_id to 1
INSERT INTO projects (id, name, description, status, metadata, user_id, created_at, updated_at)
SELECT id, name, description, status, metadata, 1, created_at, updated_at
FROM _projects_old_v40;

-- 5. drop the old table
DROP TABLE _projects_old_v40;

-- 6. recreate supporting indexes
CREATE INDEX idx_projects_status ON projects(status);
CREATE INDEX idx_projects_user ON projects(user_id);

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
