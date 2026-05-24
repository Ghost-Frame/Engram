-- Tenant schema v61: re-add user_id to conversations (reverses v37).
--
-- v16 created conversations with user_id; v37 dropped it (portability-table
-- drop) under the per-shard-only isolation assumption. The single-DB-mode
-- repair restores user_id so conversation reads/writes isolate per user in
-- every deployment mode. The monolith conversations table kept its user_id, so
-- this migration is tenant-only. In a shard the predicate is a no-op once
-- existing rows are backfilled to the shard owner -- the owner id is not known
-- to SQL-only migrations, so the runner performs that backfill after this runs.
--
-- ADD COLUMN is not idempotent, but the migration runner gates v61 to run
-- exactly once per shard via schema_migrations, and v37 guarantees the column
-- is absent at this point.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (61);

ALTER TABLE conversations ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_conversations_user ON conversations(user_id);
