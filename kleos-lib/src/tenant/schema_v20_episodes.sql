-- Tenant schema v20: episodes user_id + FTS5.
--
-- Tenant v1 created the `episodes` table but did not carry a user_id
-- column. Once kleos_lib::episodes started filtering by user_id in the
-- route helpers (list_episodes, list_episodes_by_time_range,
-- search_episodes_fts, get_episode_for_user), INSERTs against the
-- tenant shard would fail on the missing column. This migration
-- reshapes `episodes` to the monolith shape (user_id INTEGER DEFAULT 1,
-- TENANT_USERID_SHIM) and attaches the episodes_fts FTS5 virtual table
-- plus the three AFTER triggers that keep it in sync.
--
-- Safe to drop-and-recreate: tenant shards had `episodes` empty until
-- this phase because routes/episodes still targeted state.db.
--
-- Monolith reference: kleos-lib/src/db/schema_sql.rs:1105 (FTS5 + triggers).

DROP TRIGGER IF EXISTS episodes_fts_update;
DROP TRIGGER IF EXISTS episodes_fts_delete;
DROP TRIGGER IF EXISTS episodes_fts_insert;
DROP TABLE IF EXISTS episodes_fts;
DROP TABLE IF EXISTS episodes;

CREATE TABLE episodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT,
    session_id TEXT,
    agent TEXT,
    summary TEXT,
    memory_count INTEGER NOT NULL DEFAULT 0,
    embedding BLOB,
    duration_seconds INTEGER,
    fsrs_stability REAL,
    fsrs_difficulty REAL,
    fsrs_last_review_at TEXT,
    fsrs_reps INTEGER DEFAULT 0,
    decay_score REAL DEFAULT 1.0,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    started_at TEXT NOT NULL DEFAULT (datetime('now')),
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_episodes_session ON episodes(session_id);
CREATE INDEX IF NOT EXISTS idx_episodes_agent ON episodes(agent);
CREATE INDEX IF NOT EXISTS idx_episodes_user ON episodes(user_id);

CREATE VIRTUAL TABLE episodes_fts USING fts5(
    title, summary, agent,
    content='episodes', content_rowid='id',
    tokenize='porter unicode61'
);

CREATE TRIGGER episodes_fts_insert AFTER INSERT ON episodes BEGIN
    INSERT INTO episodes_fts(rowid, title, summary, agent)
    VALUES (new.id, new.title, new.summary, new.agent);
END;

CREATE TRIGGER episodes_fts_delete AFTER DELETE ON episodes BEGIN
    INSERT INTO episodes_fts(episodes_fts, rowid, title, summary, agent)
    VALUES ('delete', old.id, old.title, old.summary, old.agent);
END;

CREATE TRIGGER episodes_fts_update AFTER UPDATE ON episodes BEGIN
    INSERT INTO episodes_fts(episodes_fts, rowid, title, summary, agent)
    VALUES ('delete', old.id, old.title, old.summary, old.agent);
    INSERT INTO episodes_fts(rowid, title, summary, agent)
    VALUES (new.id, new.title, new.summary, new.agent);
END;
