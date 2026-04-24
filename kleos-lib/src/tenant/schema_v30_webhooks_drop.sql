-- Tenant schema v30: drop user_id shim from webhooks.
--
-- No UNIQUE or FK references the user_id column in tenant shards (the
-- monolith FK to users(id) was dropped in v9). idx_webhooks_user is the
-- only index that must come off before the column can be removed. The other
-- index (idx_webhooks_active on is_active) stays intact.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (30);

DROP INDEX IF EXISTS idx_webhooks_user;

ALTER TABLE webhooks DROP COLUMN user_id;
