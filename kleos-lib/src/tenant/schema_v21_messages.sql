-- Tenant schema v21: messages + messages_fts (conversations child table).
--
-- v16 added `conversations` to the tenant shard. Messages were deferred
-- until routes/conversations landed on ResolvedDb, so a tenant shard
-- had no `messages` table yet. This migration adds:
--   * messages (FK to conversations, tenant-scoped via parent)
--   * messages_fts FTS5 virtual table + 3 AFTER triggers for insert/delete/update
--
-- Monolith reference: kleos-lib/src/db/schema_sql.rs:242 (messages),
-- :1124 (messages_fts + triggers).
--
-- Safe to drop-and-recreate: the tables never existed on tenant shards.

DROP TRIGGER IF EXISTS messages_fts_update;
DROP TRIGGER IF EXISTS messages_fts_delete;
DROP TRIGGER IF EXISTS messages_fts_insert;
DROP TABLE IF EXISTS messages_fts;
DROP TABLE IF EXISTS messages;

CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_msg_conv ON messages(conversation_id, created_at);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    content, role,
    content='messages', content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, content, role)
    VALUES (new.id, new.content, new.role);
END;

CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content, role)
    VALUES ('delete', old.id, old.content, old.role);
END;

CREATE TRIGGER messages_fts_update AFTER UPDATE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, content, role)
    VALUES ('delete', old.id, old.content, old.role);
    INSERT INTO messages_fts(rowid, content, role)
    VALUES (new.id, new.content, new.role);
END;
