-- Tenant schema v16: portability family (preferences KV, conversations,
-- app_state). Supports routes/portability + kleos-lib/src/preferences.rs
-- and the export pipeline in kleos-lib/src/admin/mod.rs:export_user_data.
--
-- Monolith references:
--   * user_preferences   -- kleos-lib/src/db/schema_sql.rs:460 (KV with
--                           optional behavioral fields; UNIQUE(user_id, key))
--   * conversations      -- schema_sql.rs:226
--   * app_state          -- schema_sql.rs:1049 (global KV; routes/portability
--                           namespaces entries by key-prefix "user:{id}:")
--
-- Per TENANT_USERID_SHIM: user_id columns stay with DEFAULT 1.
-- user_preferences loses its FK to users(id) (no users table on tenant
-- shards), same treatment as webhooks.user_id in v9.
--
-- user_preferences is drop-and-recreated because tenant v1 had the old
-- behavioral (domain/preference/strength) shape that preferences.rs no
-- longer uses. The monolith merged both shapes into one table; we
-- adopt that merged shape on the tenant side too.
--
-- conversations and app_state are new on tenant shards -- routes
-- targeting them were still on state.db before v16, so tables were
-- guaranteed empty.

DROP TABLE IF EXISTS user_preferences;
DROP TABLE IF EXISTS conversations;
DROP TABLE IF EXISTS app_state;

CREATE TABLE user_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM; monolith FK dropped
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    domain TEXT,
    preference TEXT,
    strength REAL NOT NULL DEFAULT 1.0,
    evidence_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, key)
);
CREATE INDEX IF NOT EXISTS idx_user_prefs_user ON user_preferences(user_id);
CREATE INDEX IF NOT EXISTS idx_up_domain ON user_preferences(domain COLLATE NOCASE);
CREATE UNIQUE INDEX IF NOT EXISTS idx_up_domain_pref_user ON user_preferences(domain, preference, user_id);

CREATE TABLE conversations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    session_id TEXT,
    title TEXT,
    metadata TEXT,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_conversations_agent ON conversations(agent);
CREATE INDEX IF NOT EXISTS idx_conversations_session ON conversations(session_id);
CREATE INDEX IF NOT EXISTS idx_conversations_user ON conversations(user_id);
CREATE INDEX IF NOT EXISTS idx_conv_started ON conversations(started_at DESC);

-- app_state keeps the monolith "global KV" shape. Tenant scoping is
-- done in-app by prefixing keys with "user:{id}:" (see
-- routes/portability get_state/delete_state).
CREATE TABLE app_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
