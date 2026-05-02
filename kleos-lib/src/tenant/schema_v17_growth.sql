-- Tenant schema v17: growth/reflections table.
--
-- Mirrors the monolith shape at kleos-lib/src/db/schema_sql.rs:621:
--   * reflections (user_id, reflection_type, source_memory_ids, confidence)
--
-- routes/growth writes to this table via
-- kleos_lib::intelligence::growth::reflect. Before v17 the tenant shard
-- had no reflections table -- growth still targeted state.db -- so a
-- plain CREATE is safe (no data to preserve).
--
-- Per TENANT_USERID_SHIM: user_id stays with DEFAULT 1.

DROP TABLE IF EXISTS reflections;

CREATE TABLE reflections (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    content TEXT NOT NULL,
    reflection_type TEXT NOT NULL DEFAULT 'insight',
    themes TEXT,
    period_start TEXT,
    period_end TEXT,
    source_memory_ids TEXT,
    confidence REAL NOT NULL DEFAULT 1.0,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_reflections_user ON reflections(user_id);
CREATE INDEX IF NOT EXISTS idx_reflections_type ON reflections(reflection_type);
CREATE INDEX IF NOT EXISTS idx_reflections_period ON reflections(period_end DESC);
