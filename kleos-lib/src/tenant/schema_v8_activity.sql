-- Tenant schema v8: axon_events and soma_agents shim.
--
-- Mirrors the monolith tables from kleos-lib/src/db/schema_sql.rs:1214 (axon_events)
-- and :1296 (soma_agents). Missing from tenant schema_v1 entirely.
--
-- Needed for Phase 3.9 (routes/activity). `process_activity` writes to all of
-- memories (already in v1), soma_agents (get_agent_by_name/register_agent/heartbeat),
-- and axon_events (publish_event). Without these tables, any tenant shard
-- receiving POST /activity would error on the first soma or axon SQL.
--
-- Per the TENANT_USERID_SHIM policy: every row in a tenant shard carries the
-- shard owner's user_id. Column stays for now (Phase 4 drops it).
--
-- Safe to drop-and-recreate: both tables were empty on tenant shards before v8.
-- soma_groups / soma_agent_groups / soma_agent_logs / other axon_* tables are
-- deliberately left to later phases when their routes migrate.

DROP TABLE IF EXISTS axon_events;
DROP TABLE IF EXISTS soma_agents;

CREATE TABLE axon_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel TEXT NOT NULL,
    source TEXT NOT NULL,
    type TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER NOT NULL DEFAULT 1 -- TENANT_USERID_SHIM
);

CREATE INDEX IF NOT EXISTS idx_axon_events_channel ON axon_events(channel, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_axon_events_type ON axon_events(type);
CREATE INDEX IF NOT EXISTS idx_axon_events_user ON axon_events(user_id);

CREATE TABLE soma_agents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL,
    description TEXT,
    capabilities TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending','online','offline','error')),
    config TEXT NOT NULL DEFAULT '{}',
    heartbeat_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    quality_score REAL,
    drift_flags TEXT DEFAULT '[]',
    user_id INTEGER NOT NULL DEFAULT 1 -- TENANT_USERID_SHIM
);

CREATE INDEX IF NOT EXISTS idx_soma_agents_type ON soma_agents(type);
CREATE INDEX IF NOT EXISTS idx_soma_agents_status ON soma_agents(status);
CREATE INDEX IF NOT EXISTS idx_soma_agents_user ON soma_agents(user_id);
