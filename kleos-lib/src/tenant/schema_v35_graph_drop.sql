-- Tenant schema v35: drop user_id from the graph cluster (5 tenant-side tables).
--
-- Tables handled and their shapes:
--   entities           -- Shape B: UNIQUE(name, entity_type, user_id) -> UNIQUE(name, entity_type)
--   structured_facts   -- Shape A: drop user_id + user-scoped indexes
--   entity_cooccurrences -- Shape A: simple DROP COLUMN
--   memory_pagerank    -- Shape B: PK rebuild (memory_id, user_id) -> (memory_id)
--   pagerank_dirty     -- Special CHECK rebuild: singleton row pattern
--
-- NOTE: brain_edges is NOT present on tenant shards (it is a monolith-only
-- table, handled by monolith migration v38). This file intentionally omits it.
--
-- entities has FKs from memory_entities(entity_id) and
-- entity_cooccurrences(entity_a_id, entity_b_id) and
-- entity_relationships(source_entity_id, target_entity_id).
-- We set legacy_alter_table=1 so the RENAME does not rewrite those FKs
-- to point at the old table; they keep resolving by name to the rebuilt one.
--
-- memory_pagerank has FK to memories(id) ON DELETE CASCADE.
-- pagerank_dirty is a singleton-row control table; the single row is preserved
-- through the rebuild via INSERT OR IGNORE ... SELECT.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (35);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- ============================================================================
-- 1. entities: Shape B rebuild (UNIQUE constraint includes user_id)
-- ============================================================================

-- 1a. rename old table
ALTER TABLE entities RENAME TO _entities_old_v34;

-- 1b. drop indexes referencing the old shape
DROP INDEX IF EXISTS idx_entities_user;
DROP INDEX IF EXISTS idx_entities_name;
DROP INDEX IF EXISTS idx_entities_type;

-- 1c. create new table without user_id; UNIQUE collapses to (name, entity_type)
CREATE TABLE entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'concept',
    type TEXT NOT NULL DEFAULT 'generic',
    description TEXT,
    aliases TEXT,
    aka TEXT,
    metadata TEXT,
    space_id INTEGER,
    confidence REAL NOT NULL DEFAULT 1.0,
    occurrence_count INTEGER NOT NULL DEFAULT 1,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, entity_type)
);

-- 1d. copy rows forward (drop user_id); ON CONFLICT skips duplicates that
--     arise only because different user_ids mapped to the same (name, entity_type)
--     in the old multi-tenant shape.
INSERT OR IGNORE INTO entities
    (id, name, entity_type, type, description, aliases, aka, metadata,
     space_id, confidence, occurrence_count,
     first_seen_at, last_seen_at, created_at, updated_at)
SELECT
    id, name, entity_type, type, description, aliases, aka, metadata,
    space_id, confidence, occurrence_count,
    first_seen_at, last_seen_at, created_at, updated_at
FROM _entities_old_v34;

-- 1e. drop old table
DROP TABLE _entities_old_v34;

-- 1f. recreate supporting indexes
CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);

-- ============================================================================
-- 2. structured_facts: Shape A (drop user_id indexes first, then the column)
-- ============================================================================

-- Drop all indexes that reference user_id before dropping the column.
DROP INDEX IF EXISTS idx_facts_user;
DROP INDEX IF EXISTS idx_sf_subject_verb;
DROP INDEX IF EXISTS idx_facts_user_subject_predicate;

-- Recreate idx_sf_subject_verb without user_id.
CREATE INDEX IF NOT EXISTS idx_sf_subject_verb ON structured_facts(subject COLLATE NOCASE, verb);

ALTER TABLE structured_facts DROP COLUMN user_id;

-- ============================================================================
-- 3. entity_cooccurrences: Shape A (simple DROP COLUMN)
-- ============================================================================

DROP INDEX IF EXISTS idx_ec_user;
ALTER TABLE entity_cooccurrences DROP COLUMN user_id;

-- ============================================================================
-- 4. memory_pagerank: Shape B rebuild (PRIMARY KEY includes user_id)
-- ============================================================================

-- 4a. rename old table
ALTER TABLE memory_pagerank RENAME TO _memory_pagerank_old_v34;

-- 4b. drop old user index
DROP INDEX IF EXISTS idx_pagerank_user;

-- 4c. create new table: PK is memory_id alone
CREATE TABLE memory_pagerank (
    memory_id INTEGER PRIMARY KEY,
    score REAL NOT NULL,
    computed_at INTEGER NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
);

-- 4d. copy rows forward (drop user_id); ON CONFLICT skips dups from
--     same memory_id under different user_ids
INSERT OR IGNORE INTO memory_pagerank (memory_id, score, computed_at)
SELECT memory_id, score, computed_at
FROM _memory_pagerank_old_v34;

-- 4e. drop old table
DROP TABLE _memory_pagerank_old_v34;

-- 4f. recreate score index
CREATE INDEX IF NOT EXISTS idx_pagerank_score ON memory_pagerank(score DESC);

-- ============================================================================
-- 5. pagerank_dirty: CHECK constraint rebuild (singleton row pattern)
-- ============================================================================

-- 5a. rename old table
ALTER TABLE pagerank_dirty RENAME TO _pagerank_dirty_old_v34;

-- 5b. create new singleton table without user_id
--     id = 1 always; CHECK enforces the singleton invariant
CREATE TABLE pagerank_dirty (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    dirty_count INTEGER NOT NULL DEFAULT 0,
    last_refresh INTEGER NOT NULL DEFAULT 0
);

-- 5c. copy the existing row forward; OR IGNORE so an empty old table
--     does not error; seed id=1 if it does not exist
INSERT OR IGNORE INTO pagerank_dirty (id, dirty_count, last_refresh)
SELECT 1, COALESCE(dirty_count, 0), COALESCE(last_refresh, 0)
FROM _pagerank_dirty_old_v34
LIMIT 1;

-- 5d. seed the singleton row if the old table was empty
INSERT OR IGNORE INTO pagerank_dirty (id, dirty_count, last_refresh)
VALUES (1, 0, 0);

-- 5e. drop old table
DROP TABLE _pagerank_dirty_old_v34;

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
