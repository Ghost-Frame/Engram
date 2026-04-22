-- Tenant schema v10: ingestion infrastructure.
--
-- Creates the three tables routes/ingestion writes to that were missing
-- from tenant schema_v1:
--   * upload_sessions / upload_chunks (chunked upload flow), mirrored from
--     monolith migration 21 in kleos-lib/src/db/migrations.rs:1103.
--   * ingestion_hashes (per-user dedup), mirrored from
--     kleos-lib/src/db/schema_sql.rs:128.
--
-- Per TENANT_USERID_SHIM: upload_sessions and ingestion_hashes keep the
-- user_id column for Phase 4 to drop workspace-wide.
--
-- Safe to drop-and-recreate: all three tables were empty on tenant shards
-- before v10 because routes still targeted state.db.

DROP TABLE IF EXISTS upload_chunks;
DROP TABLE IF EXISTS upload_sessions;
DROP TABLE IF EXISTS ingestion_hashes;

CREATE TABLE upload_sessions (
    upload_id TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    filename TEXT,
    content_type TEXT,
    source TEXT NOT NULL DEFAULT 'upload',
    total_size INTEGER,
    total_chunks INTEGER,
    chunk_size INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    completed_at TEXT,
    expires_at TEXT NOT NULL,
    final_sha256 TEXT
);

CREATE INDEX IF NOT EXISTS idx_upload_sessions_user ON upload_sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_upload_sessions_status ON upload_sessions(status);
CREATE INDEX IF NOT EXISTS idx_upload_sessions_expires ON upload_sessions(expires_at);

CREATE TABLE upload_chunks (
    upload_id TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    chunk_hash TEXT NOT NULL,
    size INTEGER NOT NULL,
    data BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (upload_id, chunk_index),
    FOREIGN KEY (upload_id) REFERENCES upload_sessions(upload_id) ON DELETE CASCADE
);

CREATE TABLE ingestion_hashes (
    sha256 TEXT NOT NULL,
    user_id INTEGER NOT NULL DEFAULT 1, -- TENANT_USERID_SHIM
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    job_id TEXT,
    PRIMARY KEY (sha256, user_id)
);
