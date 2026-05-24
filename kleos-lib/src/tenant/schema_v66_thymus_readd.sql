-- Tenant schema v66: re-add user_id to the five thymus tables that v36 dropped:
-- rubrics, evaluations, quality_metrics, session_quality, behavioral_drift_events.
--
-- rubrics carried UNIQUE(name) before v36. Proper per-user isolation requires
-- UNIQUE(user_id, name), so a 12-step table rebuild is needed (same pattern as
-- the entities rebuild in v63). evaluations holds a FK to rubrics(id), so
-- PRAGMA foreign_keys must be toggled off for the rename/recreate.
--
-- The other four tables take a simple ALTER TABLE ADD COLUMN path. These are
-- NOT individually pragma-guarded; idempotency relies on the schema_migrations
-- version gate in the tenant migration runner (this file only executes if
-- version < 66).

INSERT OR IGNORE INTO schema_migrations (version) VALUES (66);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;

ALTER TABLE rubrics RENAME TO _rubrics_old_v66;
DROP INDEX IF EXISTS idx_rubrics_name;
DROP INDEX IF EXISTS idx_rubrics_user_name;
DROP INDEX IF EXISTS idx_rubrics_user;

CREATE TABLE rubrics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    description TEXT,
    criteria TEXT NOT NULL DEFAULT '[]',
    user_id INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO rubrics (id, name, description, criteria, user_id, created_at, updated_at)
SELECT id, name, description, criteria, 1, created_at, updated_at
FROM _rubrics_old_v66;

DROP TABLE _rubrics_old_v66;

CREATE UNIQUE INDEX IF NOT EXISTS idx_rubrics_user_name ON rubrics(user_id, name);
CREATE INDEX IF NOT EXISTS idx_rubrics_user ON rubrics(user_id);

PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;

-- Simple ADD COLUMN for the remaining four tables.
ALTER TABLE evaluations ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE quality_metrics ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE session_quality ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE behavioral_drift_events ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_evaluations_user ON evaluations(user_id);
CREATE INDEX IF NOT EXISTS idx_quality_metrics_user ON quality_metrics(user_id);
CREATE INDEX IF NOT EXISTS idx_session_quality_user ON session_quality(user_id);
CREATE INDEX IF NOT EXISTS idx_behavioral_drift_user ON behavioral_drift_events(user_id);
