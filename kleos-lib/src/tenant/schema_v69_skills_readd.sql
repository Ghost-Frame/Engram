-- Tenant schema v69: re-add user_id to skill_records via 12-step REBUILD.
-- v35 area dropped user_id; this restores it with UNIQUE(name, agent, version, user_id).
-- Also drops and recreates FTS triggers.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (69);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;

DROP TRIGGER IF EXISTS skills_fts_insert;
DROP TRIGGER IF EXISTS skills_fts_delete;
DROP TRIGGER IF EXISTS skills_fts_update;

ALTER TABLE skill_records RENAME TO _skill_records_old_v69;

DROP INDEX IF EXISTS idx_skill_records_agent;
DROP INDEX IF EXISTS idx_skill_records_name;
DROP INDEX IF EXISTS idx_skill_records_user;
DROP INDEX IF EXISTS idx_skill_records_active;
DROP INDEX IF EXISTS idx_skill_records_category;
DROP INDEX IF EXISTS idx_skill_records_parent;

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
    user_id INTEGER NOT NULL DEFAULT 1,
    kind TEXT NOT NULL DEFAULT 'skill',
    source_plugin TEXT,
    source_path TEXT,
    content_hash TEXT,
    first_seen TEXT NOT NULL DEFAULT (datetime('now')),
    last_updated TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, agent, version, user_id)
);

INSERT INTO skill_records (
    id, skill_id, name, agent, description, code, path, content,
    category, origin, generation, lineage_change_summary, creator_id,
    language, version, parent_skill_id, root_skill_id, embedding,
    embedding_vec_1024, trust_score, success_count, failure_count,
    execution_count, avg_duration_ms, is_active, is_deprecated,
    total_selections, total_applied, total_completions, visibility,
    lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
    total_fallbacks, metadata, user_id, kind, source_plugin, source_path,
    content_hash, first_seen, last_updated, created_at, updated_at
)
SELECT
    id, skill_id, name, agent, description, code, path, content,
    category, origin, generation, lineage_change_summary, creator_id,
    language, version, parent_skill_id, root_skill_id, embedding,
    embedding_vec_1024, trust_score, success_count, failure_count,
    execution_count, avg_duration_ms, is_active, is_deprecated,
    total_selections, total_applied, total_completions, visibility,
    lineage_source_task_id, lineage_content_diff, lineage_content_snapshot,
    total_fallbacks, metadata, 1, kind, source_plugin, source_path,
    content_hash, first_seen, last_updated, created_at, updated_at
FROM _skill_records_old_v69
ORDER BY id ASC;

DROP TABLE _skill_records_old_v69;

CREATE INDEX IF NOT EXISTS idx_skill_records_agent ON skill_records(agent);
CREATE INDEX IF NOT EXISTS idx_skill_records_name ON skill_records(name);
CREATE INDEX IF NOT EXISTS idx_skill_records_user ON skill_records(user_id);
CREATE INDEX IF NOT EXISTS idx_skill_records_active ON skill_records(is_active);
CREATE INDEX IF NOT EXISTS idx_skill_records_category ON skill_records(category);
CREATE INDEX IF NOT EXISTS idx_skill_records_parent ON skill_records(parent_skill_id);

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

INSERT INTO skills_fts(skills_fts) VALUES('rebuild');

PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
