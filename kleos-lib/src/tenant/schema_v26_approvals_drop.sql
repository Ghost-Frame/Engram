-- Tenant schema v26: drop user_id shim from approvals.
--
-- No UNIQUE or FK references the column, so the drop is a plain
-- DROP INDEX + ALTER TABLE DROP COLUMN. Both the simple
-- idx_approvals_user index and the composite idx_approvals_user_status
-- index must go before the column can be dropped.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (26);

DROP INDEX IF EXISTS idx_approvals_user;
DROP INDEX IF EXISTS idx_approvals_user_status;

ALTER TABLE approvals DROP COLUMN user_id;
