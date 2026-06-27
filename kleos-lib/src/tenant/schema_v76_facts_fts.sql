-- Tenant schema v76: facts_fts FTS5 index over structured_facts (L5 facts channel).
--
-- structured_facts already carries subject/predicate/object/verb and user_id on the current
-- tenant shape (re-added by v67; the full SPO+verb shape comes from the v14 graph schema), so
-- this migration only adds the external-content FTS index and its sync triggers, then rebuilds
-- from existing rows. No user_id backfill is needed here -- v67 already backfilled
-- structured_facts.user_id to the shard owner.
--
-- The virtual table is external-content (content='structured_facts', content_rowid='id'):
-- application code must never INSERT/UPDATE/DELETE facts_fts directly; the triggers are the
-- sole sync mechanism. All statements are IF NOT EXISTS / idempotent.

CREATE VIRTUAL TABLE IF NOT EXISTS facts_fts USING fts5(
    subject, predicate, object, verb,
    content='structured_facts',
    content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS facts_fts_insert AFTER INSERT ON structured_facts BEGIN
    INSERT INTO facts_fts(rowid, subject, predicate, object, verb)
    VALUES (new.id, new.subject, new.predicate, new.object, new.verb);
END;

CREATE TRIGGER IF NOT EXISTS facts_fts_delete AFTER DELETE ON structured_facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, predicate, object, verb)
    VALUES ('delete', old.id, old.subject, old.predicate, old.object, old.verb);
END;

CREATE TRIGGER IF NOT EXISTS facts_fts_update AFTER UPDATE ON structured_facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, subject, predicate, object, verb)
    VALUES ('delete', old.id, old.subject, old.predicate, old.object, old.verb);
    INSERT INTO facts_fts(rowid, subject, predicate, object, verb)
    VALUES (new.id, new.subject, new.predicate, new.object, new.verb);
END;

INSERT INTO facts_fts(facts_fts) VALUES ('rebuild');
