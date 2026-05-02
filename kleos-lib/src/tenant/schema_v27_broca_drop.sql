-- Tenant schema v27: drop user_id shim from broca_actions.
--
-- No UNIQUE or FK references the column, so the drop is a plain
-- DROP INDEX + ALTER TABLE DROP COLUMN. The other (agent/service/
-- action/created_at) indexes stay intact.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (27);

DROP INDEX IF EXISTS idx_broca_actions_user;

ALTER TABLE broca_actions DROP COLUMN user_id;
