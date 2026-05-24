-- Tenant schema v63: re-add user_id to the graph entities table with
-- UNIQUE(name, entity_type, user_id), reversing the v35 graph-cluster drop.
--
-- v34 created entities with user_id + UNIQUE(name, entity_type, user_id); v35
-- dropped user_id under the per-shard-only isolation assumption, collapsing the
-- constraint to UNIQUE(name, entity_type). The single-DB-mode repair restores
-- user_id so entities isolate per user in every deployment mode -- without it,
-- two users mentioning the same name collapse into one shared entity row.
-- Changing a UNIQUE constraint requires the 12-step rebuild (mirrors monolith
-- migration 72).
--
-- entities is FK-referenced by entity_relationships, memory_entities, and
-- entity_cooccurrences; the rebuild preserves id values and runs with
-- PRAGMA foreign_keys = OFF so those references stay valid. The runner
-- backfills the copied DEFAULT-1 rows to the shard owner after this file runs.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (63);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

ALTER TABLE entities RENAME TO _entities_old_v63;

DROP INDEX IF EXISTS idx_entities_name;
DROP INDEX IF EXISTS idx_entities_type;

CREATE TABLE entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'concept',
    type TEXT NOT NULL DEFAULT 'generic',
    description TEXT,
    aliases TEXT,
    aka TEXT,
    metadata TEXT,
    user_id INTEGER NOT NULL DEFAULT 1,
    space_id INTEGER,
    confidence REAL NOT NULL DEFAULT 1.0,
    occurrence_count INTEGER NOT NULL DEFAULT 1,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, entity_type, user_id)
);

INSERT OR IGNORE INTO entities
    (id, name, entity_type, type, description, aliases, aka, metadata,
     user_id, space_id, confidence, occurrence_count,
     first_seen_at, last_seen_at, created_at, updated_at)
SELECT
    id, name, entity_type, type, description, aliases, aka, metadata,
    1, space_id, confidence, occurrence_count,
    first_seen_at, last_seen_at, created_at, updated_at
FROM _entities_old_v63;

DROP TABLE _entities_old_v63;

CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
CREATE INDEX IF NOT EXISTS idx_entities_user ON entities(user_id);

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
