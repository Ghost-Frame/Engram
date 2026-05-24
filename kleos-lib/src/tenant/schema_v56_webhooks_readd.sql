-- Tenant schema v56: re-add user_id to webhooks (reverses v30).
--
-- v30 dropped user_id from the shard webhooks table on the assumption that the
-- per-shard file is the only isolation boundary. The single-DB-mode repair
-- restores user_id as a universal, always-applied predicate so webhook reads
-- and deliveries isolate correctly in every deployment mode. In a shard the
-- predicate is a no-op once existing rows are backfilled to the shard owner --
-- the owner id is not known to SQL-only migrations, so the runner
-- (run_tenant_migrations) performs that backfill after this file runs.
--
-- ADD COLUMN is not idempotent, but the migration runner gates v56 to run
-- exactly once per shard via schema_migrations, and v30 guarantees the column
-- is absent at this point.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (56);

ALTER TABLE webhooks ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_webhooks_user ON webhooks(user_id);
