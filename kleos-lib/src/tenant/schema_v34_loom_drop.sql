-- Tenant schema v34: drop user_id from loom_workflows (UNIQUE rebuild) and
-- loom_runs (simple DROP COLUMN).
--
-- loom_workflows was created with UNIQUE(user_id, name). SQLite cannot drop a
-- column that participates in a UNIQUE constraint, so we follow the 12-step
-- table-rebuild pattern matching v28 projects.
--
-- The new constraint is UNIQUE(name). In a single-tenant shard every row
-- carries user_id=1, so the old (user_id, name) collapsed to (name);
-- the new constraint is equivalent on surviving rows.
--
-- loom_runs has a FK to loom_workflows(id) ON DELETE CASCADE. We set
-- legacy_alter_table=1 so the RENAME of loom_workflows does not rewrite
-- that FK to point at the temporary old table. loom_runs then gets a
-- simple DROP COLUMN for its own user_id.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (34);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- loom_workflows: Shape B rebuild --

-- 1. rename out of the way
ALTER TABLE loom_workflows RENAME TO _loom_workflows_old_v33;

-- 2. drop indexes that reference the old column / table shape
DROP INDEX IF EXISTS idx_loom_workflows_user;

-- 3. create the new table without user_id
CREATE TABLE loom_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    steps TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name)
);

-- 4. copy rows forward, dropping the user_id column
INSERT INTO loom_workflows (id, name, description, steps, created_at, updated_at)
SELECT id, name, description, steps, created_at, updated_at
FROM _loom_workflows_old_v33;

-- 5. drop the old table
DROP TABLE _loom_workflows_old_v33;

-- loom_runs: Shape A DROP COLUMN --

-- 6. drop the user index on loom_runs before dropping the column
DROP INDEX IF EXISTS idx_loom_runs_user;

-- 7. drop user_id from loom_runs
ALTER TABLE loom_runs DROP COLUMN user_id;

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
