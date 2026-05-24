-- Tenant schema v55: re-add user_id to memory core tables (reverses v22).
--
-- v22 dropped user_id from memories/artifacts/vector_sync_pending on the
-- assumption that the per-shard file is the only isolation boundary. The
-- single-DB-mode repair restores user_id as a universal, always-applied
-- predicate so isolation is correct in every deployment mode. In a shard the
-- predicate is a no-op once existing rows are backfilled to the shard owner's
-- user_id -- but the owner id is not known to SQL-only migrations, so the
-- runner (run_tenant_migrations) performs that backfill after this file runs.
-- This file only does the owner-independent work: add the columns (defaulting
-- to 1), recreate the indexes v22 dropped, and restore the trigger.
--
-- structured_facts on the tenant side never had user_id (see v22), so it is
-- intentionally skipped here.
--
-- ADD COLUMN is not idempotent, but the migration runner gates v55 to run
-- exactly once per shard via schema_migrations, and v22 guarantees the columns
-- are absent at this point.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (55);

ALTER TABLE memories ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE artifacts ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE vector_sync_pending ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_memories_user_id ON memories(user_id);
CREATE INDEX IF NOT EXISTS idx_artifacts_user ON artifacts(user_id);

-- Block links between memories owned by different users (defense in depth;
-- a no-op within a correctly single-owner shard, a real guard if a shard is
-- ever cross-contaminated).
CREATE TRIGGER IF NOT EXISTS prevent_cross_tenant_links
    BEFORE INSERT ON memory_links
    BEGIN
        SELECT RAISE(ABORT, 'cross-tenant memory links are not permitted')
        WHERE (SELECT user_id FROM memories WHERE id = NEW.source_id)
           != (SELECT user_id FROM memories WHERE id = NEW.target_id);
    END;
