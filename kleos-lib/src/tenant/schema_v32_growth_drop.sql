-- Tenant schema v32: drop user_id shim from reflections.
--
-- No UNIQUE or FK references the user_id column. idx_reflections_user is
-- the only user-specific index; idx_reflections_type and
-- idx_reflections_period stay intact.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (32);

DROP INDEX IF EXISTS idx_reflections_user;

ALTER TABLE reflections DROP COLUMN user_id;
