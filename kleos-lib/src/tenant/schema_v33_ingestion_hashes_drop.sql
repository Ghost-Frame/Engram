-- Tenant schema v33: drop user_id from ingestion_hashes (PK rebuild).
--
-- v10 created ingestion_hashes with PRIMARY KEY (sha256, user_id). SQLite
-- cannot drop a column that participates in a PRIMARY KEY, so we follow
-- the 12-step table-rebuild pattern matching v28 projects.
--
-- The new PK is (sha256) alone. In a single-tenant shard every row has
-- user_id=1, so the old (sha256, user_id) collapsed to (sha256); the new
-- constraint is equivalent on surviving rows. INSERT OR IGNORE is used in
-- the row copy in case multiple rows somehow share the same sha256 under
-- the old composite PK (dedup is defensive, not expected in practice).
--
-- No FK references ingestion_hashes from other tables.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (33);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- 1. rename out of the way
ALTER TABLE ingestion_hashes RENAME TO _ingestion_hashes_old_v32;

-- 2. no extra indexes on ingestion_hashes to drop beyond the implicit PK index

-- 3. create the new table without user_id
CREATE TABLE ingestion_hashes (
    sha256 TEXT NOT NULL,
    first_seen_at TEXT NOT NULL DEFAULT (datetime('now')),
    job_id TEXT,
    PRIMARY KEY (sha256)
);

-- 4. copy rows forward, dropping the user_id column
--    INSERT OR IGNORE deduplicates if multiple rows shared the same sha256
--    under the old composite PK (first row wins, which is fine for dedup).
INSERT OR IGNORE INTO ingestion_hashes (sha256, first_seen_at, job_id)
SELECT sha256, first_seen_at, job_id
FROM _ingestion_hashes_old_v32;

-- 5. drop the old table
DROP TABLE _ingestion_hashes_old_v32;

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
