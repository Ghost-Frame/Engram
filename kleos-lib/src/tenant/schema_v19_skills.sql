-- Tenant schema v19: skills family.
--
-- Monolith references (kleos-lib/src/db/schema_sql.rs):
--   * skill_records         -- :691
--   * skill_lineage_parents -- :742
--   * skill_tags            -- :750
--   * execution_analyses    -- :760
--   * skill_judgments       -- :775
--   * skill_tool_deps       -- :786
--   * tool_quality_records  -- :798
--   * skills_fts + triggers -- :1143 (FTS5 virtual table + 3 triggers)
--
-- Per TENANT_USERID_SHIM: skill_records.user_id stays with DEFAULT 1.
-- All tables were absent on tenant shards before v19 (routes/skills
-- still targeted state.db), so drop-and-recreate is safe.

DROP TRIGGER IF EXISTS skills_fts_update;
DROP TRIGGER IF EXISTS skills_fts_delete;
DROP TRIGGER IF EXISTS skills_fts_insert;
DROP TABLE IF EXISTS skills_fts;
DROP TABLE IF EXISTS tool_quality_records;
DROP TABLE IF EXISTS skill_tool_deps;
DROP TABLE IF EXISTS skill_judgments;
DROP TABLE IF EXISTS execution_analyses;
DROP TABLE IF EXISTS skill_tags;
DROP TABLE IF EXISTS skill_lineage_parents;
DROP TABLE IF EXISTS skill_records;

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
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    first_seen TEXT NOT NULL DEFAULT (datetime('now')),
    last_updated TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(name, agent, version, user_id)
);
CREATE INDEX IF NOT EXISTS idx_skill_records_agent ON skill_records(agent);
CREATE INDEX IF NOT EXISTS idx_skill_records_name ON skill_records(name);
CREATE INDEX IF NOT EXISTS idx_skill_records_user ON skill_records(user_id);
CREATE INDEX IF NOT EXISTS idx_skill_records_active ON skill_records(is_active);
CREATE INDEX IF NOT EXISTS idx_skill_records_category ON skill_records(category);
CREATE INDEX IF NOT EXISTS idx_skill_records_parent ON skill_records(parent_skill_id);

CREATE TABLE skill_lineage_parents (
    skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
    parent_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
    parent_skill_id TEXT,
    PRIMARY KEY (skill_id, parent_id)
);

CREATE TABLE skill_tags (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    UNIQUE(skill_id, tag)
);
CREATE INDEX IF NOT EXISTS idx_skill_tags_skill ON skill_tags(skill_id);
CREATE INDEX IF NOT EXISTS idx_skill_tags_tag ON skill_tags(tag);

CREATE TABLE execution_analyses (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
    success BOOLEAN NOT NULL,
    duration_ms REAL,
    error_type TEXT,
    error_message TEXT,
    input_hash TEXT,
    output_hash TEXT,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_exec_analyses_skill ON execution_analyses(skill_id);

CREATE TABLE skill_judgments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
    judge_agent TEXT NOT NULL,
    score REAL NOT NULL,
    rationale TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_skill_judgments_skill ON skill_judgments(skill_id);

CREATE TABLE skill_tool_deps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    skill_id INTEGER NOT NULL REFERENCES skill_records(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    tool_key TEXT,
    critical INTEGER NOT NULL DEFAULT 0,
    is_optional BOOLEAN NOT NULL DEFAULT 0,
    UNIQUE(skill_id, tool_name)
);
CREATE INDEX IF NOT EXISTS idx_skill_tool_deps_skill ON skill_tool_deps(skill_id);

CREATE TABLE tool_quality_records (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tool_key TEXT UNIQUE,
    backend TEXT NOT NULL DEFAULT '',
    server TEXT NOT NULL DEFAULT 'default',
    tool_name TEXT NOT NULL,
    description_hash TEXT NOT NULL DEFAULT '',
    total_calls INTEGER NOT NULL DEFAULT 0,
    total_successes INTEGER NOT NULL DEFAULT 0,
    total_failures INTEGER NOT NULL DEFAULT 0,
    avg_execution_ms REAL NOT NULL DEFAULT 0,
    llm_flagged_count INTEGER NOT NULL DEFAULT 0,
    quality_score REAL NOT NULL DEFAULT 1.0,
    last_execution_at TEXT,
    agent TEXT NOT NULL,
    success BOOLEAN NOT NULL,
    latency_ms REAL,
    error_type TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_tool_quality_tool ON tool_quality_records(tool_name);

CREATE VIRTUAL TABLE skills_fts USING fts5(
    name, description, code,
    content='skill_records', content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER skills_fts_insert AFTER INSERT ON skill_records BEGIN
    INSERT INTO skills_fts(rowid, name, description, code)
    VALUES (new.id, new.name, new.description, new.code);
END;

CREATE TRIGGER skills_fts_delete AFTER DELETE ON skill_records BEGIN
    INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
    VALUES ('delete', old.id, old.name, old.description, old.code);
END;

CREATE TRIGGER skills_fts_update AFTER UPDATE ON skill_records BEGIN
    INSERT INTO skills_fts(skills_fts, rowid, name, description, code)
    VALUES ('delete', old.id, old.name, old.description, old.code);
    INSERT INTO skills_fts(rowid, name, description, code)
    VALUES (new.id, new.name, new.description, new.code);
END;
