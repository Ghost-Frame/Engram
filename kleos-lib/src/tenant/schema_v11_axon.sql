-- Tenant schema v11: remaining axon tables.
--
-- v8 added axon_events as a prerequisite for routes/activity. This migration
-- completes the axon family for routes/axon:
--   * axon_channels (kleos-lib/src/db/schema_sql.rs:1206)
--   * axon_subscriptions (same file, :1227)
--   * axon_cursors (same file, :1239)
--
-- Per TENANT_USERID_SHIM: axon_subscriptions and axon_cursors keep their
-- user_id column with default 1 so Phase 4 can drop it workspace-wide.
-- axon_channels has no user_id in the monolith; left alone here too.
--
-- Safe to drop-and-recreate: all three tables were empty on tenant shards
-- before v11 because routes still targeted state.db.

DROP TABLE IF EXISTS axon_cursors;
DROP TABLE IF EXISTS axon_subscriptions;
DROP TABLE IF EXISTS axon_channels;

CREATE TABLE axon_channels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    retain_hours INTEGER NOT NULL DEFAULT 168
);

CREATE TABLE axon_subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    channel TEXT NOT NULL,
    filter_type TEXT,
    webhook_url TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    UNIQUE(agent, channel)
);

CREATE INDEX IF NOT EXISTS idx_axon_subs_channel ON axon_subscriptions(channel);

CREATE TABLE axon_cursors (
    agent TEXT NOT NULL,
    channel TEXT NOT NULL,
    last_event_id INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    PRIMARY KEY(agent, channel)
);
