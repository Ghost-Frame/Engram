-- Tenant schema v59: re-add user_id to axon_events (reverses v29).
--
-- v8 created axon_events with user_id; v29 dropped it under the per-shard-only
-- isolation assumption. The single-DB-mode repair restores user_id so event
-- reads (get/query/consume/stats) isolate per user in every deployment mode.
-- In a shard the predicate is a no-op once existing rows are backfilled to the
-- shard owner -- the owner id is not known to SQL-only migrations, so the
-- runner (run_tenant_migrations) performs that backfill after this file runs.
--
-- ADD COLUMN is not idempotent, but the migration runner gates v59 to run
-- exactly once per shard via schema_migrations, and v29 guarantees the column
-- is absent at this point.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (59);

ALTER TABLE axon_events ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_axon_events_user ON axon_events(user_id, channel, id);
