-- Tenant schema v39: drop user_id from skill_records (Shape B + FTS shadow rebuild).
--
-- skill_records has an in-table UNIQUE(name, agent, version, user_id) which
-- prevents simple DROP COLUMN. We do a full table rebuild (Shape B):
--
-- New in-table constraint: UNIQUE(name, agent, version).
-- Drop INDEX idx_skill_records_user (user-scoped plain index).
-- Preserve INDEX idx_skill_records_agent, idx_skill_records_name,
--   idx_skill_records_active, idx_skill_records_category, idx_skill_records_parent.
--
-- FTS handling:
--   1. Drop the 3 FTS triggers bound to skill_records.
--   2. Rename skill_records out of the way.
--   3. Drop the user-scoped index.
--   4. Create the new table (no user_id, new UNIQUE).
--   5. INSERT ... SELECT preserving id values so FK references stay valid.
--   6. Drop the old table.
--   7. Recreate the 5 preserved indexes.
--   8. Recreate the 3 FTS triggers verbatim (their bodies never referenced user_id).
--   9. INSERT INTO skills_fts(skills_fts) VALUES('rebuild') to refresh the shadow.
--
-- Child FK fanout (all ON DELETE CASCADE, preserved by legacy_alter_table=1):
--   skill_lineage_parents, skill_tags, execution_analyses, skill_judgments,
--   skill_tool_deps.
-- Self-references: parent_skill_id and root_skill_id REFERENCES skill_records(id).

INSERT OR IGNORE INTO schema_migrations (version) VALUES (39);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- ============================================================================
-- Step 1: drop FTS triggers (bound to skill_records; must be removed before rename)
-- ============================================================================

DROP TRIGGER IF EXISTS skills_fts_insert;
DROP TRIGGER IF EXISTS skills_fts_delete;
DROP TRIGGER IF EXISTS skills_fts_update;

-- ============================================================================
-- Step 2: rename the old table out of the way
-- ============================================================================

ALTER TABLE skill_records RENAME TO _skill_records_old_v38;

-- ============================================================================
-- Step 3: drop indexes that reference the old table or old shape
-- ============================================================================

DROP INDEX IF EXISTS idx_skill_records_user;
DROP INDEX IF EXISTS idx_skill_records_agent;
DROP INDEX IF EXISTS idx_skill_records_name;
DROP INDEX IF EXISTS idx_skill_records_active;
DROP INDEX IF EXISTS idx_skill_records_category;
DROP INDEX IF EXISTS idx_skill_records_parent;

-- ============================================================================
-- Step 4: create the new table without user_id
-- ============================================================================

CREATE TABLE skill_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    skill_id TEXT UNIQUE,
    name TEXT NOT NULL,
    agent TEXT NOT NULL,
    description TEXT,
    code TEXT NOT NULL,
    path TEXT,
    content TEXT NOT NULL DEFAULT '',
    category TEXT NOT NULL DEFAULT 'workflow',
    origin TEXT NOT NULL DEFAULT 'imported',
    generation INTEGER NOT NULL DEFAULT 0,
    lineage_change_summary TEXT,
    creator_id TEXT,
    language TEXT NOT NULL DEFAULT 'javascript',
    version INTEGER NOT NULL DEFAULT 1,
    parent_skill_id INTEGER REFERENCES skill_records(id),
    root_skill_id INTEGER REFERENCES skill_records(id),
    embedding BLOB,
    embedding_vec_1024 FLOAT32(1024),
    trust_score REAL NOT NULL DEFAULT 50,
    success_count INTEGER NOT NULL DEFAULT 0,
    failure_count INTEGER NOT NULL DEFAULT 0,
    execution_count INTEGER NOT NULL DEFAULT 0,
    avg_duration_ms REAL,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    is_deprecated BOOLEAN NOT NULL DEFAULT 0,
    total_selections INTEGER NOT NULL DEFAULT 0,
    total_applied INTEGER NOT NULL DEFAULT 0,
    total_completions INTEGER NOT NULL DEFAULT 0,
    visibility TEXT NOT NULL DEFAULT 'private',
    lineage_source_task_id TEXT,
    lineage_content_diff TEXT NOT NULL DEFAULT '',
    lineage_content_snapshot TEXT NOT NULL DEFAULT '{}',
    total_fallbacks INTEGER NOT NULL DEFAULT 0,
    metadata TEXT,
    first_seen TEXT NOT NULL DEFAULT (datetime('now')),
    last_updated TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, agent, version)
);

-- ============================================================================
-- Step 5: copy rows forward preserving id values so FK references stay valid.
-- On conflict (rows that differed only by user_id now share the same
-- (name, agent, version) triple), keep the row with the lower id (first written).
-- ============================================================================

INSERT OR IGNORE INTO skill_records (
    id, skill_id, name, agent, description, code, path, content, category, origin,
    generation, lineage_change_summary, creator_id, language, version,
    parent_skill_id, root_skill_id, embedding, embedding_vec_1024,
    trust_score, success_count, failure_count, execution_count, avg_duration_ms,
    is_active, is_deprecated, total_selections, total_applied, total_completions,
    visibility, lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
    total_fallbacks, metadata, first_seen, last_updated, created_at, updated_at
)
SELECT
    id, skill_id, name, agent, description, code, path, content, category, origin,
    generation, lineage_change_summary, creator_id, language, version,
    parent_skill_id, root_skill_id, embedding, embedding_vec_1024,
    trust_score, success_count, failure_count, execution_count, avg_duration_ms,
    is_active, is_deprecated, total_selections, total_applied, total_completions,
    visibility, lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
    total_fallbacks, metadata, first_seen, last_updated, created_at, updated_at
FROM _skill_records_old_v38
ORDER BY id ASC;

-- ============================================================================
-- Step 6: drop the old table
-- ============================================================================

DROP TABLE _skill_records_old_v38;

-- ============================================================================
-- Step 7: recreate the 5 preserved indexes
-- ============================================================================

CREATE INDEX IF NOT EXISTS idx_skill_records_agent ON skill_records(agent);
CREATE INDEX IF NOT EXISTS idx_skill_records_name ON skill_records(name);
CREATE INDEX IF NOT EXISTS idx_skill_records_active ON skill_records(is_active);
CREATE INDEX IF NOT EXISTS idx_skill_records_category ON skill_records(category);
CREATE INDEX IF NOT EXISTS idx_skill_records_parent ON skill_records(parent_skill_id);

-- ============================================================================
-- Step 8: recreate the 3 FTS triggers verbatim (their bodies never referenced user_id)
-- ============================================================================

CREATE TRIGGER IF NOT EXISTS skills_fts_insert AFTER INSERT ON skill_records BEGIN
    INSERT INTO skills_fts(rowid, name, description, code)
    VALUES (new.id, new.name, new.description, new.code);
END;

CREATE TRIGGER IF NOT EXISTS skills_fts_delete AFTER DELETE ON skill_records BEGIN
    INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
    VALUES ('delete', old.id, old.name, old.description, old.code);
END;

CREATE TRIGGER IF NOT EXISTS skills_fts_update AFTER UPDATE ON skill_records BEGIN
    INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
    VALUES ('delete', old.id, old.name, old.description, old.code);
    INSERT INTO skills_fts(rowid, name, description, code)
    VALUES (new.id, new.name, new.description, new.code);
END;

-- ============================================================================
-- Step 9: rebuild the FTS shadow from the new skill_records content
-- ============================================================================

INSERT INTO skills_fts(skills_fts) VALUES('rebuild');

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
