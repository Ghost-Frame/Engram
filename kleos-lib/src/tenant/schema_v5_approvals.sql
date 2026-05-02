-- Tenant schema v5: approvals shim.
--
-- Mirrors the monolith `approvals` table (kleos-lib/src/db/migrations.rs:838,
-- migration 12). Missing from tenant schema_v1 entirely, so until this
-- migration ran, `routes/approvals` could only succeed on the monolith
-- fallback.
--
-- Per the TENANT_USERID_SHIM policy: every row in a tenant shard carries the
-- shard owner's user_id. The column stays for now so the kleos-lib approvals
-- module does not need SQL changes this phase. Phase 4 will drop it
-- workspace-wide.
--
-- Safe to drop-and-recreate: table was empty on tenant shards before v5
-- because the routes still targeted state.db.

DROP TABLE IF EXISTS approvals;

CREATE TABLE approvals (
    id TEXT PRIMARY KEY,
    action TEXT NOT NULL,
    context TEXT,
    requester TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    decision_by TEXT,
    decision_reason TEXT,
    created_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    decided_at TEXT,
    user_id INTEGER NOT NULL DEFAULT 1 -- TENANT_USERID_SHIM
);

CREATE INDEX IF NOT EXISTS idx_approvals_status ON approvals(status);
CREATE INDEX IF NOT EXISTS idx_approvals_expires ON approvals(expires_at);
CREATE INDEX IF NOT EXISTS idx_approvals_user ON approvals(user_id);
CREATE INDEX IF NOT EXISTS idx_approvals_user_status ON approvals(user_id, status);
