-- Tenant schema v43: fold session-handoff storage into the tenant shard.
--
-- The standalone data_dir/handoffs.db is replaced by a per-shard handoffs
-- table set. The reserved tenant id "handoffs" is intended to hold all
-- session-handoff rows for every authenticated user (Master's agents and
-- the bot all POST to /handoffs/* on the same backing shard), so the
-- table KEEPS its user_id column. Other tenants get the table too, which
-- is harmless: only the "handoffs" tenant is wired through /handoffs/*.
--
-- Mirrors kleos_lib::handoffs::SCHEMA_SQL plus the runtime-applied user_id
-- index from setup_schema(), so existing HandoffsDb queries keep working
-- against a tenant-resident table.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (43);

CREATE TABLE IF NOT EXISTS handoffs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'utc')),
    project TEXT NOT NULL,
    branch TEXT,
    directory TEXT,
    agent TEXT DEFAULT 'unknown',
    type TEXT DEFAULT 'manual',
    content TEXT NOT NULL,
    metadata TEXT,
    session_id TEXT,
    model TEXT,
    host TEXT,
    content_hash TEXT
);

CREATE INDEX IF NOT EXISTS idx_handoffs_project ON handoffs(project, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_created ON handoffs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_hash ON handoffs(content_hash);
CREATE INDEX IF NOT EXISTS idx_handoffs_agent ON handoffs(agent, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_type ON handoffs(type, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_session ON handoffs(session_id);
CREATE INDEX IF NOT EXISTS idx_handoffs_model ON handoffs(model, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_restore ON handoffs(project, type, agent, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_user_created ON handoffs(user_id, created_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS handoffs_fts USING fts5(
    content, content='handoffs', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS handoffs_fts_ai AFTER INSERT ON handoffs BEGIN
    INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS handoffs_fts_ad AFTER DELETE ON handoffs BEGIN
    INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER IF NOT EXISTS handoffs_fts_au AFTER UPDATE OF content ON handoffs BEGIN
    INSERT INTO handoffs_fts(handoffs_fts, rowid, content) VALUES('delete', old.id, old.content);
    INSERT INTO handoffs_fts(rowid, content) VALUES (new.id, new.content);
END;
