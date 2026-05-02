-- Tenant schema v28: drop user_id from projects (UNIQUE rebuild).
--
-- v7 created projects with UNIQUE(name, user_id). SQLite's ALTER TABLE
-- DROP COLUMN cannot remove a column that participates in a UNIQUE
-- constraint, so we follow the 12-step table-rebuild pattern matching
-- v23 scratchpad.
--
-- The new constraint is UNIQUE(name). In a single-tenant shard every row
-- carries user_id=1, so the old (name, user_id) collapsed to (name);
-- the new constraint is equivalent on surviving rows.
--
-- memory_projects has a FK to projects(id). We set legacy_alter_table=1
-- so the RENAME does not rewrite that FK to point at the temporary old
-- table; the FK keeps referencing "projects" by name and naturally
-- resolves to the newly-created table. foreign_keys is disabled for the
-- duration of the rebuild.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (28);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- 1. rename out of the way
ALTER TABLE projects RENAME TO _projects_old_v27;

-- 2. drop indexes that reference the old column / table shape
DROP INDEX IF EXISTS idx_projects_user;
DROP INDEX IF EXISTS idx_projects_status;

-- 3. create the new table without user_id
CREATE TABLE projects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name)
);

-- 4. copy rows forward, dropping the user_id column
INSERT INTO projects (id, name, description, status, metadata, created_at, updated_at)
SELECT id, name, description, status, metadata, created_at, updated_at
FROM _projects_old_v27;

-- 5. drop the old table
DROP TABLE _projects_old_v27;

-- 6. recreate supporting indexes without user_id
CREATE INDEX idx_projects_status ON projects(status);

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
