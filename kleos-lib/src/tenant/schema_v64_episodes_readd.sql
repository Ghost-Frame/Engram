-- Tenant schema v64: re-add user_id to episodes (reverses v40).
--
-- v20 created episodes with user_id; v40 dropped it (Shape A) under the
-- per-shard-only isolation assumption. The single-DB-mode repair restores
-- user_id so episode reads/writes isolate per user in every deployment mode.
-- episodes has no in-table UNIQUE involving user_id, so this is a simple
-- ADD COLUMN + index re-add. The runner backfills existing rows to the shard
-- owner after this runs.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (64);

ALTER TABLE episodes ADD COLUMN user_id INTEGER DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_episodes_user ON episodes(user_id);
