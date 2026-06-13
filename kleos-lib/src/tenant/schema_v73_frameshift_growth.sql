-- Tenant schema v73: Frameshift cross-machine growth log.
--
-- Mirrors the handoffs model (schema_v43): a single reserved tenant
-- ("frameshift-growth") holds every authenticated user's growth entries in one
-- shared shard, row-scoped by user_id. All of one operator's machines
-- authenticate as the same user (see identity_keys), so they converge on one
-- logical growth set. Other tenants get the table too (harmless); only the
-- reserved "frameshift-growth" tenant is wired through /frameshift-growth/*.
--
-- Append-only, deduped on content_hash (UNIQUE -> idempotent POST). No decay,
-- no consolidation, no GC -- growth entries are durable by design. The
-- autoincrement id is the monotonic since-cursor for incremental pull.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (73);

CREATE TABLE IF NOT EXISTS frameshift_growth (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
    persona TEXT,
    project_id TEXT,
    scope TEXT,
    content TEXT NOT NULL,
    metadata TEXT,
    host TEXT,
    content_hash TEXT NOT NULL,
    UNIQUE(user_id, content_hash)
);

CREATE INDEX IF NOT EXISTS idx_fsgrowth_user_cursor ON frameshift_growth(user_id, id);
CREATE INDEX IF NOT EXISTS idx_fsgrowth_persona ON frameshift_growth(user_id, persona, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_fsgrowth_project ON frameshift_growth(user_id, project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_fsgrowth_scope ON frameshift_growth(user_id, scope, created_at DESC);
