-- Tenant schema v24: drop user_id shim from sessions.
--
-- sessions carries user_id only as the TENANT_USERID_SHIM. No UNIQUE or
-- FOREIGN KEY constraint references the column, so the drop is a plain
-- DROP INDEX + ALTER TABLE DROP COLUMN.
--
-- session_output never had user_id on either tenant or monolith, so it is
-- intentionally untouched.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (24);

DROP INDEX IF EXISTS idx_sessions_user;

ALTER TABLE sessions DROP COLUMN user_id;
