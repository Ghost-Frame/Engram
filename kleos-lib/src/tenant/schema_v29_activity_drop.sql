-- Tenant schema v29: drop user_id shim from axon_events and soma_agents.
--
-- No UNIQUE or FK references the user_id column on either table.
-- idx_axon_events_user and idx_soma_agents_user are plain single-column
-- indexes; they must drop before the column can be removed. Other indexes
-- (channel/type on axon_events; type/status on soma_agents) stay intact.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (29);

DROP INDEX IF EXISTS idx_axon_events_user;

ALTER TABLE axon_events DROP COLUMN user_id;

DROP INDEX IF EXISTS idx_soma_agents_user;

ALTER TABLE soma_agents DROP COLUMN user_id;
