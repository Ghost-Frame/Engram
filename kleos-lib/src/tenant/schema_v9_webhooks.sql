-- Tenant schema v9: webhooks and webhook_dead_letters shim.
--
-- Mirrors the monolith tables from kleos-lib/src/db/schema_sql.rs:254
-- (webhooks) and :1369 (webhook_dead_letters). Missing from tenant
-- schema_v1 entirely.
--
-- The monolith webhooks table declares `user_id REFERENCES users(id)`.
-- Tenant shards do not carry the users table (users is system-only and
-- lives in the main DB), so the FK is dropped here. The column stays as
-- a plain INTEGER with the TENANT_USERID_SHIM default so Phase 4 can drop
-- it workspace-wide.
--
-- Safe to drop-and-recreate: both tables were empty on tenant shards
-- before v9 because routes still targeted state.db.

DROP TABLE IF EXISTS webhook_dead_letters;
DROP TABLE IF EXISTS webhooks;

CREATE TABLE webhooks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM, was FK to users(id)
    url TEXT NOT NULL,
    events TEXT NOT NULL DEFAULT 'memory.created',
    secret TEXT,
    is_active BOOLEAN NOT NULL DEFAULT 1,
    active BOOLEAN NOT NULL DEFAULT 1,
    last_triggered_at TEXT,
    failure_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_webhooks_user ON webhooks(user_id);
CREATE INDEX IF NOT EXISTS idx_webhooks_active ON webhooks(is_active);

CREATE TABLE webhook_dead_letters (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    webhook_id INTEGER NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
    event TEXT NOT NULL,
    payload TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_status_code INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_wdl_webhook ON webhook_dead_letters(webhook_id, created_at);
