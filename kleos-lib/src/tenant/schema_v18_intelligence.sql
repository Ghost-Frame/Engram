-- Tenant schema v18: intelligence family (consolidations, current_state,
-- causal chains/links, reconsolidations, temporal_patterns, digests,
-- memory_feedback).
--
-- Monolith references (kleos-lib/src/db/schema_sql.rs):
--   * consolidations      -- :313
--   * current_state       -- :440 (per-agent KV; used by fact extraction)
--   * causal_chains       -- :498
--   * causal_links        -- :509
--   * reconsolidations    -- :521
--   * temporal_patterns   -- :534
--   * digests             -- :547
--   * memory_feedback     -- :609
--
-- Per TENANT_USERID_SHIM: user_id columns stay with DEFAULT 1.
-- Safe to drop-and-recreate: all eight tables were absent on tenant
-- shards before v18 because routes/intelligence still targeted state.db.

DROP TABLE IF EXISTS memory_feedback;
DROP TABLE IF EXISTS digests;
DROP TABLE IF EXISTS temporal_patterns;
DROP TABLE IF EXISTS reconsolidations;
DROP TABLE IF EXISTS causal_links;
DROP TABLE IF EXISTS causal_chains;
DROP TABLE IF EXISTS current_state;
DROP TABLE IF EXISTS consolidations;

CREATE TABLE consolidations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source_ids TEXT NOT NULL,
    result_memory_id INTEGER REFERENCES memories(id),
    summary_memory_id INTEGER REFERENCES memories(id),
    source_memory_ids TEXT,
    cluster_label TEXT,
    strategy TEXT NOT NULL DEFAULT 'merge',
    confidence REAL NOT NULL DEFAULT 1.0,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_consolidations_user ON consolidations(user_id);

CREATE TABLE current_state (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
    previous_value TEXT,
    previous_memory_id INTEGER,
    updated_count INTEGER NOT NULL DEFAULT 1,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(agent, key, user_id)
);
CREATE INDEX IF NOT EXISTS idx_current_state_agent ON current_state(agent);
CREATE INDEX IF NOT EXISTS idx_current_state_user ON current_state(user_id);
CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);
CREATE UNIQUE INDEX IF NOT EXISTS idx_cs_key_user ON current_state(key, user_id);

CREATE TABLE causal_chains (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    root_memory_id INTEGER REFERENCES memories(id),
    description TEXT,
    confidence REAL NOT NULL DEFAULT 1.0,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_causal_chains_user ON causal_chains(user_id);

CREATE TABLE causal_links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chain_id INTEGER NOT NULL REFERENCES causal_chains(id) ON DELETE CASCADE,
    cause_memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    effect_memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    strength REAL NOT NULL DEFAULT 1.0,
    order_index INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_causal_links_chain ON causal_links(chain_id);

CREATE TABLE reconsolidations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    old_content TEXT NOT NULL,
    new_content TEXT NOT NULL,
    reason TEXT,
    triggered_by INTEGER REFERENCES memories(id),
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_reconsolidations_memory ON reconsolidations(memory_id);

CREATE TABLE temporal_patterns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    pattern_type TEXT NOT NULL DEFAULT 'daily',
    description TEXT NOT NULL,
    memory_ids TEXT,
    confidence REAL NOT NULL DEFAULT 1.0,
    recurrence TEXT,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_temporal_patterns_user ON temporal_patterns(user_id);

CREATE TABLE digests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    period TEXT NOT NULL DEFAULT 'daily',
    content TEXT NOT NULL,
    memory_count INTEGER NOT NULL DEFAULT 0,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    schedule TEXT NOT NULL DEFAULT 'daily',
    webhook_url TEXT,
    webhook_secret TEXT,
    include_stats BOOLEAN NOT NULL DEFAULT 1,
    include_new_memories BOOLEAN NOT NULL DEFAULT 1,
    include_contradictions BOOLEAN NOT NULL DEFAULT 1,
    include_reflections BOOLEAN NOT NULL DEFAULT 1,
    last_sent_at TEXT,
    next_send_at TEXT,
    active BOOLEAN NOT NULL DEFAULT 1,
    failure_count INTEGER NOT NULL DEFAULT 0,
    started_at TEXT,
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_digests_user ON digests(user_id);
CREATE INDEX IF NOT EXISTS idx_digests_period ON digests(period);
CREATE INDEX IF NOT EXISTS idx_digests_next ON digests(next_send_at) WHERE active = 1;

CREATE TABLE memory_feedback (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    user_id INTEGER NOT NULL, -- TENANT_USERID_SHIM
    rating TEXT NOT NULL,
    context TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_feedback_user ON memory_feedback(user_id);
CREATE INDEX IF NOT EXISTS idx_feedback_memory ON memory_feedback(memory_id);
