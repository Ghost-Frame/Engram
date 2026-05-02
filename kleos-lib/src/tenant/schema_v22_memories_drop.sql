-- Tenant schema v22: drop user_id from memory core tables.
--
-- Each tenant shard is single-owner, so user_id on memories/artifacts/
-- vector_sync_pending is a redundant shim (TENANT_USERID_SHIM). This
-- migration removes those columns and the indexes that covered them.
--
-- structured_facts on the tenant side never had a user_id column, so it is
-- intentionally skipped here.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (22);

-- Drop the shim index before dropping the column it covers.
DROP INDEX IF EXISTS idx_memories_user_id;
DROP INDEX IF EXISTS idx_artifacts_user;

-- Drop FTS triggers so we can recreate them cleanly (bodies are unchanged
-- but we drop+recreate to avoid any stale references).
DROP TRIGGER IF EXISTS memories_fts_insert;
DROP TRIGGER IF EXISTS memories_fts_delete;
DROP TRIGGER IF EXISTS memories_fts_update;

-- Drop user_id from the three shim tables.
ALTER TABLE memories DROP COLUMN user_id;
ALTER TABLE artifacts DROP COLUMN user_id;
ALTER TABLE vector_sync_pending DROP COLUMN user_id;

-- Recreate FTS triggers (exact bodies from schema_v1.sql lines 308-319).
CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE OF content ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
END;
