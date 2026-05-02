-- Tenant schema v6: broca_actions shim.
--
-- Mirrors the monolith `broca_actions` table from
-- kleos-lib/src/db/schema_sql.rs:1249. Missing from tenant schema_v1,
-- which means any tenant-shard code path that calls
-- `kleos_lib::services::broca::log_action` would error out on insert.
--
-- The most visible path that exercises this is `credd.resolve_text`, which
-- logs every secret resolution via `audit_resolution` -> `log_action`. That
-- runs in routes/prompts (Phase 3.7) whenever a prompt contains a
-- `{SECRET_...}` placeholder. Making `broca_actions` tenant-local is the
-- prerequisite for running credd resolution against the tenant DB.
--
-- Per the TENANT_USERID_SHIM policy: every row in a tenant shard carries the
-- shard owner's user_id. The column stays for now so kleos-lib services do
-- not need SQL changes this phase. Phase 4 drops it workspace-wide.
--
-- Safe to drop-and-recreate: the table was empty on tenant shards before v6
-- because every code path that touched it still targeted state.db.

DROP TABLE IF EXISTS broca_actions;

CREATE TABLE broca_actions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    service TEXT NOT NULL,
    action TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '{}',
    narrative TEXT,
    axon_event_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER NOT NULL DEFAULT 1 -- TENANT_USERID_SHIM
);

CREATE INDEX IF NOT EXISTS idx_broca_actions_agent ON broca_actions(agent, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_broca_actions_service ON broca_actions(service, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_broca_actions_action ON broca_actions(action, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_broca_actions_created ON broca_actions(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_broca_actions_user ON broca_actions(user_id);
