-- Tenant schema v68: re-add user_id to user_preferences via 12-step REBUILD.
-- v37 dropped user_id and changed UNIQUE(user_id, key) to UNIQUE(key).
-- This restores user_id with UNIQUE(user_id, key) so single-DB mode can
-- isolate preferences per user.
--
-- Also restores idx_up_domain_pref_user on (domain, preference, user_id).

INSERT OR IGNORE INTO schema_migrations (version) VALUES (68);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = ON;

ALTER TABLE user_preferences RENAME TO _user_preferences_old_v68;
DROP INDEX IF EXISTS idx_up_domain;
DROP INDEX IF EXISTS idx_up_domain_pref;
DROP INDEX IF EXISTS idx_up_domain_pref_user;
DROP INDEX IF EXISTS idx_user_prefs_user;

CREATE TABLE user_preferences (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
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

INSERT INTO user_preferences
    (id, user_id, key, value, domain, preference, strength,
     evidence_memory_id, created_at, updated_at)
SELECT
    id, 1, key, value, domain, preference, strength,
    evidence_memory_id, created_at, updated_at
FROM _user_preferences_old_v68;

DROP TABLE _user_preferences_old_v68;

CREATE INDEX IF NOT EXISTS idx_user_prefs_user ON user_preferences(user_id);
CREATE INDEX IF NOT EXISTS idx_up_domain ON user_preferences(domain COLLATE NOCASE);
CREATE UNIQUE INDEX IF NOT EXISTS idx_up_domain_pref_user ON user_preferences(domain, preference, user_id);

PRAGMA legacy_alter_table = OFF;
PRAGMA foreign_keys = ON;
