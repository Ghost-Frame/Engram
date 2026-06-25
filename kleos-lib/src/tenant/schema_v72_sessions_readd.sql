-- Tenant schema v72: re-add user_id to sessions (reverses v24).
--
-- v24 dropped user_id from sessions on the per-shard-file isolation
-- assumption. The single-DB-mode repair restores user_id as a universal,
-- always-applied predicate so session read/enumerate is correct in every
-- deployment mode. In a shard the predicate is a no-op once existing rows are
-- backfilled to the shard owner's user_id -- the runner (run_tenant_migrations)
-- performs that backfill after this file runs (see
-- TENANT_MIGRATION_READD_USER_ID_SESSIONS / backfill_owner_tables_for_version).
--
-- sessions carries no UNIQUE or FOREIGN KEY on user_id (v24 confirmed this), so
-- the readd is a plain ADD COLUMN + recreate the index v24 dropped. session_output
-- never had user_id on either side and is intentionally untouched.
--
-- ADD COLUMN is not idempotent, but the runner gates v72 to run exactly once per
-- shard via schema_migrations, and v24 guarantees the column is absent here.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (72);

ALTER TABLE sessions ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
