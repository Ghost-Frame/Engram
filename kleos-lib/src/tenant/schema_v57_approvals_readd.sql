-- Tenant schema v57: re-add user_id to approvals (reverses v26).
--
-- v26 dropped user_id from the shard approvals table under the per-shard-only
-- isolation assumption. The single-DB-mode repair restores user_id as a
-- universal, always-applied predicate so approvals isolate correctly in every
-- deployment mode. In a shard the predicate is a no-op once existing rows are
-- backfilled to the shard owner -- the owner id is not known to SQL-only
-- migrations, so the runner (run_tenant_migrations) performs that backfill
-- after this file runs.
--
-- ADD COLUMN is not idempotent, but the migration runner gates v57 to run
-- exactly once per shard via schema_migrations, and v26 guarantees the column
-- is absent at this point.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (57);

ALTER TABLE approvals ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_approvals_user ON approvals(user_id);
CREATE INDEX IF NOT EXISTS idx_approvals_user_status ON approvals(user_id, status);
