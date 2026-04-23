-- Tenant schema v14: graph family (entities, relationships, cooccurrences,
-- memory-entity join, pagerank, structured facts).
--
-- Aligns tenant-shard graph tables with the monolith shapes that
-- routes/graph and kleos-lib/src/graph/** expect. Tenant v1 had stale
-- pre-refactor shapes for entities / entity_relationships /
-- structured_facts / memory_pagerank; monolith has since evolved. This
-- migration drop-and-recreates those four to the current runtime shape,
-- and adds memory_entities, entity_cooccurrences, pagerank_dirty which
-- never existed on the tenant side.
--
-- Monolith reference points:
--   * entities                -- schema_sql.rs:328
--   * entity_relationships    -- schema_sql.rs:352
--   * memory_entities         -- schema_sql.rs:367
--   * structured_facts        -- schema_sql.rs:405
--   * entity_cooccurrences    -- schema_sql.rs:478
--   * memory_pagerank / pagerank_dirty -- migrations.rs:669 / :679
--
-- Per TENANT_USERID_SHIM: every user_id column stays with DEFAULT 1.
-- Phase 4 will drop these columns once the monolith bypass is gone.
--
-- Safe to drop-and-recreate: every reshaped table was empty on tenant
-- shards before v14 because routes/graph still targeted state.db.

-- Drop children first (FKs reference parents). Children of
-- memory_pagerank / entity_cooccurrences / memory_entities are empty,
-- so drop in dependency order.
DROP TABLE IF EXISTS pagerank_dirty;
DROP TABLE IF EXISTS memory_pagerank;
DROP TABLE IF EXISTS entity_cooccurrences;
DROP TABLE IF EXISTS memory_entities;
DROP TABLE IF EXISTS structured_facts;
DROP TABLE IF EXISTS entity_relationships;
DROP TABLE IF EXISTS entities;

CREATE TABLE entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    entity_type TEXT NOT NULL DEFAULT 'concept',
    type TEXT NOT NULL DEFAULT 'generic',
    description TEXT,
    aliases TEXT,
    aka TEXT,
    metadata TEXT,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    space_id INTEGER,
    confidence REAL NOT NULL DEFAULT 1.0,
    occurrence_count INTEGER NOT NULL DEFAULT 1,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, entity_type, user_id)
);
CREATE INDEX IF NOT EXISTS idx_entities_name ON entities(name);
CREATE INDEX IF NOT EXISTS idx_entities_type ON entities(entity_type);
CREATE INDEX IF NOT EXISTS idx_entities_user ON entities(user_id);

CREATE TABLE entity_relationships (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    target_entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    relationship_type TEXT NOT NULL DEFAULT 'related',
    relationship TEXT NOT NULL DEFAULT 'related',
    strength REAL NOT NULL DEFAULT 1.0,
    evidence_count INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(source_entity_id, target_entity_id, relationship_type)
);
CREATE INDEX IF NOT EXISTS idx_entity_rel_source ON entity_relationships(source_entity_id);
CREATE INDEX IF NOT EXISTS idx_entity_rel_target ON entity_relationships(target_entity_id);

CREATE TABLE memory_entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    entity_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    salience REAL NOT NULL DEFAULT 1.0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(memory_id, entity_id)
);
CREATE INDEX IF NOT EXISTS idx_memory_entities_memory ON memory_entities(memory_id);
CREATE INDEX IF NOT EXISTS idx_memory_entities_entity ON memory_entities(entity_id);

CREATE TABLE structured_facts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,
    subject TEXT NOT NULL,
    predicate TEXT NOT NULL,
    object TEXT NOT NULL,
    verb TEXT NOT NULL DEFAULT '',
    quantity REAL,
    unit TEXT,
    date_ref TEXT,
    date_approx TEXT,
    location TEXT,
    context TEXT,
    episode_id INTEGER,
    valid_at TEXT,
    invalid_at TEXT,
    invalidated_by INTEGER,
    confidence REAL NOT NULL DEFAULT 1.0,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_facts_subject ON structured_facts(subject);
CREATE INDEX IF NOT EXISTS idx_facts_predicate ON structured_facts(predicate);
CREATE INDEX IF NOT EXISTS idx_facts_memory ON structured_facts(memory_id);
CREATE INDEX IF NOT EXISTS idx_facts_user ON structured_facts(user_id);
CREATE INDEX IF NOT EXISTS idx_sf_verb ON structured_facts(verb);
CREATE INDEX IF NOT EXISTS idx_sf_date ON structured_facts(date_approx);
CREATE INDEX IF NOT EXISTS idx_sf_episode ON structured_facts(episode_id) WHERE episode_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sf_location ON structured_facts(location COLLATE NOCASE) WHERE location IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sf_valid ON structured_facts(valid_at) WHERE valid_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sf_invalid ON structured_facts(invalid_at) WHERE invalid_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_sf_subject_verb ON structured_facts(subject COLLATE NOCASE, verb, user_id);
CREATE INDEX IF NOT EXISTS idx_facts_user_subject_predicate ON structured_facts(user_id, subject, predicate);

CREATE TABLE entity_cooccurrences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_a_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    entity_b_id INTEGER NOT NULL REFERENCES entities(id) ON DELETE CASCADE,
    count INTEGER NOT NULL DEFAULT 1,
    cooccurrence_count INTEGER NOT NULL DEFAULT 1,
    score REAL NOT NULL DEFAULT 0.0,
    last_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
    user_id INTEGER DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(entity_a_id, entity_b_id)
);
CREATE INDEX IF NOT EXISTS idx_cooccurrences_a ON entity_cooccurrences(entity_a_id);
CREATE INDEX IF NOT EXISTS idx_cooccurrences_b ON entity_cooccurrences(entity_b_id);
CREATE INDEX IF NOT EXISTS idx_ec_score ON entity_cooccurrences(score DESC);
CREATE INDEX IF NOT EXISTS idx_ec_user ON entity_cooccurrences(user_id);

-- PageRank snapshot (monolith shape: memory_id PK, unix-timestamp computed_at).
CREATE TABLE memory_pagerank (
    memory_id INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL, -- TENANT_USERID_SHIM (required by graph::pagerank filters)
    score REAL NOT NULL,
    computed_at INTEGER NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_pagerank_user ON memory_pagerank(user_id);
CREATE INDEX IF NOT EXISTS idx_pagerank_score ON memory_pagerank(score DESC);

-- PageRank dirty-bit tracker (per user -- used to coalesce recompute triggers).
CREATE TABLE pagerank_dirty (
    user_id INTEGER PRIMARY KEY,
    dirty_count INTEGER NOT NULL DEFAULT 0,
    last_refresh INTEGER NOT NULL DEFAULT 0
);
