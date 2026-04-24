-- Tenant schema v31: drop user_id shim from axon_subscriptions and axon_cursors.
--
-- UNIQUE(agent, channel) on axon_subscriptions does NOT include user_id, so
-- plain DROP COLUMN works. PRIMARY KEY(agent, channel) on axon_cursors
-- likewise. No idx_*_user indexes exist on either table.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (31);

ALTER TABLE axon_subscriptions DROP COLUMN user_id;

ALTER TABLE axon_cursors DROP COLUMN user_id;
