-- Tenant schema v25: drop user_id shim from chiasm_tasks + chiasm_task_updates.
--
-- Neither table has a UNIQUE or FK on user_id, so the drop is a plain
-- DROP INDEX + ALTER TABLE DROP COLUMN. chiasm_task_updates never had a
-- user_id index in v4, so only the column drop is needed there.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (25);

DROP INDEX IF EXISTS idx_chiasm_tasks_user;

ALTER TABLE chiasm_tasks DROP COLUMN user_id;
ALTER TABLE chiasm_task_updates DROP COLUMN user_id;
