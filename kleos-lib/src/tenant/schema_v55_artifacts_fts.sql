-- Tenant schema v55: artifacts FTS5 index.
--
-- Tenant DBs were created without an artifacts FTS table; only the legacy
-- main-DB schema (schema_sql.rs) carried it. As a result every artifact
-- inserted into a tenant shard since the per-tenant split has been
-- unsearchable. This migration creates the missing virtual table + maintenance
-- triggers, then backfills FTS rows for any existing artifacts that already
-- have indexable `content` populated.
--
-- The virtual table is external-content (`content='artifacts',
-- content_rowid='id'`), so the triggers ARE the canonical sync mechanism --
-- application code must NOT issue its own INSERT/UPDATE/DELETE against
-- `artifacts_fts`. See `kleos-lib/src/artifacts.rs` for the application-side
-- contract.

CREATE VIRTUAL TABLE IF NOT EXISTS artifacts_fts USING fts5(
    name,
    content,
    content='artifacts',
    content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS artifacts_fts_insert AFTER INSERT ON artifacts BEGIN
    INSERT INTO artifacts_fts(rowid, name, content)
    VALUES (new.id, new.name, new.content);
END;

CREATE TRIGGER IF NOT EXISTS artifacts_fts_delete AFTER DELETE ON artifacts BEGIN
    INSERT INTO artifacts_fts(artifacts_fts, rowid, name, content)
    VALUES ('delete', old.id, old.name, old.content);
END;

CREATE TRIGGER IF NOT EXISTS artifacts_fts_update AFTER UPDATE ON artifacts BEGIN
    INSERT INTO artifacts_fts(artifacts_fts, rowid, name, content)
    VALUES ('delete', old.id, old.name, old.content);
    INSERT INTO artifacts_fts(rowid, name, content)
    VALUES (new.id, new.name, new.content);
END;

-- Backfill existing artifacts so prior uploads become searchable. Uses the
-- FTS5 `rebuild` command which fully reconstructs the index from the
-- underlying `artifacts` table -- idempotent under re-run (unlike a raw
-- INSERT...SELECT, which would duplicate rows if the table was already
-- populated out-of-band).
INSERT INTO artifacts_fts(artifacts_fts) VALUES ('rebuild');
