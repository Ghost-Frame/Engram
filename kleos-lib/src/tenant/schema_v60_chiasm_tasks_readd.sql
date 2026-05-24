-- Tenant schema v60: re-add user_id to chiasm_tasks (reverses v25).
--
-- v4 created chiasm_tasks with user_id; v25 dropped it under the
-- per-shard-only isolation assumption. The single-DB-mode repair restores
-- user_id so task reads/writes isolate per user in every deployment mode. In a
-- shard the predicate is a no-op once existing rows are backfilled to the shard
-- owner -- the owner id is not known to SQL-only migrations, so the runner
-- (run_tenant_migrations) performs that backfill after this file runs.
--
-- ADD COLUMN is not idempotent, but the migration runner gates v60 to run
-- exactly once per shard via schema_migrations, and v25 guarantees the column
-- is absent at this point.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (60);

ALTER TABLE chiasm_tasks ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_chiasm_tasks_user ON chiasm_tasks(user_id, status);
