-- Tenant schema v77: rebuild the external-content FTS5 tables with the
-- language-neutral `unicode61 remove_diacritics 2` tokenizer so non-English
-- content (German umlauts/ess-zett, French accents) is folded for keyword match
-- and English-only porter stemming is dropped. Mirrors the global migration 94
-- and the schema_sql.rs change.
--
-- Each virtual table is dropped and recreated with the new tokenizer, then
-- rebuilt from its content table. The base-table `*_fts_insert/delete/update`
-- triggers live on the content tables (memories/episodes/messages/skill_records/
-- artifacts/structured_facts), not on the virtual tables, so they survive the
-- DROP and keep syncing -- no trigger statements are needed here. Column lists
-- are the verbatim tenant shapes (note: tenant memories_fts is single-column
-- `content`, unlike the global table's content/category/source).

DROP TABLE IF EXISTS memories_fts;
CREATE VIRTUAL TABLE memories_fts USING fts5(
    content,
    content='memories',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
INSERT INTO memories_fts(memories_fts) VALUES('rebuild');

DROP TABLE IF EXISTS episodes_fts;
CREATE VIRTUAL TABLE episodes_fts USING fts5(
    title, summary, agent,
    content='episodes', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
INSERT INTO episodes_fts(episodes_fts) VALUES('rebuild');

DROP TABLE IF EXISTS messages_fts;
CREATE VIRTUAL TABLE messages_fts USING fts5(
    content, role,
    content='messages', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
INSERT INTO messages_fts(messages_fts) VALUES('rebuild');

DROP TABLE IF EXISTS skills_fts;
CREATE VIRTUAL TABLE skills_fts USING fts5(
    name, description, code,
    content='skill_records', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
INSERT INTO skills_fts(skills_fts) VALUES('rebuild');

DROP TABLE IF EXISTS artifacts_fts;
CREATE VIRTUAL TABLE artifacts_fts USING fts5(
    name, content,
    content='artifacts', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
INSERT INTO artifacts_fts(artifacts_fts) VALUES('rebuild');

DROP TABLE IF EXISTS facts_fts;
CREATE VIRTUAL TABLE facts_fts USING fts5(
    subject, predicate, object, verb,
    content='structured_facts', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);
INSERT INTO facts_fts(facts_fts) VALUES('rebuild');
