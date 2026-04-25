-- Tenant schema v40: drop user_id from episodes (Shape A).
--
-- episodes has no in-table UNIQUE that includes user_id, so this is a
-- simple DROP INDEX + ALTER TABLE DROP COLUMN (Shape A).
--
-- FTS handling: episodes_fts triggers reference (id, title, summary, agent)
-- and never referenced user_id, so the FTS shadow remains valid through the
-- column drop without any rebuild.
--
-- The episodes_vec_1024_idx libsql vector index over embedding_vec_1024 is
-- also unaffected.
--
-- Dropped: INDEX idx_episodes_user ON episodes(user_id).
-- Preserved: idx_episodes_session, idx_episodes_agent.
-- FTS triggers and shadow: untouched.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (40);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

DROP INDEX IF EXISTS idx_episodes_user;

ALTER TABLE episodes DROP COLUMN user_id;

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
