-- Tenant schema v37: drop user_id from user_preferences (Shape B + UNIQUE INDEX swap)
-- and conversations (Shape A).
--
-- user_preferences was created in v16 with UNIQUE(user_id, key) as an in-table
-- constraint plus separate indexes. SQLite cannot DROP COLUMN when that column
-- participates in an in-table UNIQUE constraint, so we follow the 12-step
-- table-rebuild pattern.
--
-- New in-table constraint: UNIQUE(key). Single-tenant shard; equivalent semantics.
-- Drop UNIQUE INDEX idx_up_domain_pref_user (was on (domain, preference, user_id)).
-- Drop INDEX idx_user_prefs_user (user-scoped plain index).
-- Recreate UNIQUE INDEX idx_up_domain_pref on (domain, preference) -- no user_id.
-- Preserve INDEX idx_up_domain(domain COLLATE NOCASE) -- not user_id-scoped.
-- Preserve FK evidence_memory_id -> memories(id) ON DELETE SET NULL.
-- Drop the user_id REFERENCES users(id) FK along with the column.
--
-- conversations was created in v16 with a simple non-UNIQUE user_id column.
-- Shape A applies: drop idx_conversations_user, then DROP COLUMN user_id.
-- messages.conversation_id FK to conversations(id) is unaffected.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (37);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- ============================================================================
-- 1. user_preferences: Shape B rebuild
-- ============================================================================

-- 1a. rename out of the way
ALTER TABLE user_preferences RENAME TO _user_preferences_old_v36;

-- 1b. drop indexes that reference the old column / table shape
DROP INDEX IF EXISTS idx_up_domain_pref_user;
DROP INDEX IF EXISTS idx_user_prefs_user;
-- idx_up_domain does not reference user_id; drop it here so it doesn't
-- collide with the new CREATE INDEX below (the name is reused unchanged).
DROP INDEX IF EXISTS idx_up_domain;

-- 1c. create the new table without user_id
CREATE TABLE user_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    domain TEXT,
    preference TEXT,
    strength REAL NOT NULL DEFAULT 1.0,
    evidence_memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(key)
);

-- 1d. copy rows forward, dropping user_id
INSERT INTO user_preferences (id, key, value, domain, preference, strength, evidence_memory_id, created_at, updated_at)
SELECT id, key, value, domain, preference, strength, evidence_memory_id, created_at, updated_at
FROM _user_preferences_old_v36;

-- 1e. drop the old table
DROP TABLE _user_preferences_old_v36;

-- 1f. recreate indexes (domain-only plain index restored; new domain+pref unique index)
CREATE INDEX IF NOT EXISTS idx_up_domain ON user_preferences(domain COLLATE NOCASE);
CREATE UNIQUE INDEX IF NOT EXISTS idx_up_domain_pref ON user_preferences(domain, preference);

-- ============================================================================
-- 2. conversations: Shape A DROP COLUMN
-- ============================================================================

-- 2a. drop the user index on conversations before dropping the column
DROP INDEX IF EXISTS idx_conversations_user;

-- 2b. drop user_id from conversations
ALTER TABLE conversations DROP COLUMN user_id;

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
