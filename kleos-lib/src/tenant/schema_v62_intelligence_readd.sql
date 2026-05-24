-- Tenant schema v62: re-add user_id to the intelligence tables
-- (reverses v32 for reflections and v38 for consolidations / causal_chains).
--
-- v17 created reflections with user_id; v32 dropped it. v18 created
-- consolidations and causal_chains with user_id; v38 dropped both (and
-- deliberately left causal_links without a user_id column). The single-DB-mode
-- repair restores user_id on these three so intelligence reads/writes isolate
-- per user in every deployment mode. causal_links stays unscoped at the column
-- level and is reached only through its parent chain's owner.
--
-- In a shard the predicate is a no-op once existing rows are backfilled to the
-- shard owner -- the owner id is not known to SQL-only migrations, so the
-- runner performs that backfill after this runs.
--
-- ADD COLUMN is not idempotent, but the migration runner gates v62 to run
-- exactly once per shard via schema_migrations, and v32/v38 guarantee the
-- columns are absent at this point.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (62);

ALTER TABLE reflections ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE consolidations ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE causal_chains ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_reflections_user ON reflections(user_id);
CREATE INDEX IF NOT EXISTS idx_consolidations_user ON consolidations(user_id);
CREATE INDEX IF NOT EXISTS idx_causal_chains_user ON causal_chains(user_id);
