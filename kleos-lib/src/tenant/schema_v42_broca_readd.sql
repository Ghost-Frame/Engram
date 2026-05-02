-- Tenant schema v42: re-add user_id to broca_actions (C-R3-004 / H-R3-006).
--
-- v27 dropped user_id from broca_actions under the same Phase-5 assumption
-- as projects. The R-3 audit (H-R3-006) showed broca routes still hit the
-- monolith state.db, so cross-tenant poisoning was reachable until those
-- routes are moved to ResolvedDb. Re-adding the column keeps shard and
-- monolith schemas aligned and supports the upcoming H-R3-006 fix.
--
-- broca_actions has no UNIQUE/FK on the column, so a plain ADD COLUMN
-- works. Legacy rows backfill to user_id=1. The new column is indexed
-- so per-user queries do not full-scan.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (42);

ALTER TABLE broca_actions ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS idx_broca_actions_user ON broca_actions(user_id, created_at DESC);
