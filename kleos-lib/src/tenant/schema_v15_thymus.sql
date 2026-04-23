-- Tenant schema v15: thymus quality scoring tables.
--
-- Mirrors the monolith shapes at kleos-lib/src/db/schema_sql.rs:913-981:
--   * rubrics (user_id, UNIQUE(user_id, name))
--   * evaluations (rubric_id FK, user_id)
--   * quality_metrics (user_id)
--   * session_quality (user_id)
--   * behavioral_drift_events (user_id)
--
-- Per TENANT_USERID_SHIM: user_id stays with DEFAULT 1 (DEFAULT 0 on
-- session_quality and behavioral_drift_events, matching the monolith
-- schema). Phase 4 will strip these columns.
--
-- Safe to drop-and-recreate: every table was empty on tenant shards
-- before v15 because routes/thymus still targeted state.db.

DROP TABLE IF EXISTS behavioral_drift_events;
DROP TABLE IF EXISTS session_quality;
DROP TABLE IF EXISTS quality_metrics;
DROP TABLE IF EXISTS evaluations;
DROP TABLE IF EXISTS rubrics;

CREATE TABLE rubrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    criteria TEXT NOT NULL DEFAULT '[]',
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_rubrics_user_name ON rubrics(user_id, name);
CREATE INDEX IF NOT EXISTS idx_rubrics_user ON rubrics(user_id);

CREATE TABLE evaluations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    rubric_id INTEGER NOT NULL REFERENCES rubrics(id),
    agent TEXT NOT NULL,
    subject TEXT NOT NULL,
    input TEXT NOT NULL DEFAULT '{}',
    output TEXT NOT NULL DEFAULT '{}',
    scores TEXT NOT NULL DEFAULT '{}',
    overall_score REAL NOT NULL,
    notes TEXT,
    evaluator TEXT NOT NULL,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_evaluations_agent_created ON evaluations(agent, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_evaluations_rubric_created ON evaluations(rubric_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_evaluations_user ON evaluations(user_id);

CREATE TABLE quality_metrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    metric TEXT NOT NULL,
    value REAL NOT NULL,
    tags TEXT NOT NULL DEFAULT '{}',
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_quality_metrics_agent_metric ON quality_metrics(agent, metric, recorded_at DESC);
CREATE INDEX IF NOT EXISTS idx_quality_metrics_user ON quality_metrics(user_id);

CREATE TABLE session_quality (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    agent TEXT NOT NULL,
    turn_count INTEGER DEFAULT 0,
    rules_followed TEXT DEFAULT '[]',
    rules_drifted TEXT DEFAULT '[]',
    personality_score REAL,
    rule_compliance_rate REAL,
    user_id INTEGER NOT NULL DEFAULT 0, -- TENANT_USERID_SHIM
    created_at TEXT DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_session_quality_user ON session_quality(user_id);

CREATE TABLE behavioral_drift_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    session_id TEXT,
    drift_type TEXT NOT NULL,
    severity TEXT DEFAULT 'low',
    signal TEXT NOT NULL,
    user_id INTEGER NOT NULL DEFAULT 0, -- TENANT_USERID_SHIM
    created_at TEXT DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_behavioral_drift_user ON behavioral_drift_events(user_id);
