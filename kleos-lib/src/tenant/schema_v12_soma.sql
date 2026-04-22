-- Tenant schema v12: remaining soma tables.
--
-- v8 added soma_agents (routes/activity heartbeats into it). v12 completes
-- the soma family for routes/soma:
--   * soma_groups (kleos-lib/src/db/schema_sql.rs:1316)
--   * soma_agent_groups (same file, :1325)
--   * soma_agent_logs (same file, :1332)
--
-- Per TENANT_USERID_SHIM: soma_groups keeps user_id with the default. The
-- two join tables have no user_id column in the monolith; left alone here
-- too (they join to soma_agents which owns the tenant attribution).
--
-- Safe to drop-and-recreate: all three tables were empty on tenant shards
-- before v12 because routes still targeted state.db.

DROP TABLE IF EXISTS soma_agent_logs;
DROP TABLE IF EXISTS soma_agent_groups;
DROP TABLE IF EXISTS soma_groups;

CREATE TABLE soma_groups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER NOT NULL DEFAULT 1 -- TENANT_USERID_SHIM
);

CREATE INDEX IF NOT EXISTS idx_soma_groups_user ON soma_groups(user_id);

CREATE TABLE soma_agent_groups (
    agent_id INTEGER NOT NULL REFERENCES soma_agents(id) ON DELETE CASCADE,
    group_id INTEGER NOT NULL REFERENCES soma_groups(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(agent_id, group_id)
);

CREATE TABLE soma_agent_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id INTEGER NOT NULL REFERENCES soma_agents(id) ON DELETE CASCADE,
    level TEXT NOT NULL DEFAULT 'info',
    message TEXT NOT NULL,
    data TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_soma_agent_logs_agent_created ON soma_agent_logs(agent_id, created_at);
