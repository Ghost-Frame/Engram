-- Tenant schema v38: drop user_id from the intelligence family (7 tables).
--
-- causal_links has NO user_id column and is excluded from this stage.
-- causal_links.chain_id -> causal_chains(id) FK is unaffected.
--
-- current_state is Shape B: the in-table UNIQUE(agent, key, user_id)
-- prevents simple DROP COLUMN, so we do the 12-step table rebuild.
-- New in-table constraint: UNIQUE(agent, key).
-- Drop UNIQUE INDEX idx_cs_key_user (key, user_id) -- superseded by the
-- in-table UNIQUE on (agent, key) which already prevents duplicates per
-- agent+key pair. A separate UNIQUE on (key) alone would be too tight
-- since multiple agents may share a key name.
-- Drop INDEX idx_current_state_user.
-- Preserve INDEX idx_current_state_agent and idx_cs_key (key COLLATE NOCASE).
--
-- All other 6 tables are Shape A: drop the user-scoped index (if any),
-- then DROP COLUMN user_id.
--
--   consolidations    -- drop idx_consolidations_user; DROP COLUMN user_id.
--   causal_chains     -- drop idx_causal_chains_user; DROP COLUMN user_id.
--   reconsolidations  -- no user-scoped index; DROP COLUMN user_id.
--   temporal_patterns -- drop idx_temporal_patterns_user; DROP COLUMN user_id.
--   digests           -- drop idx_digests_user; DROP COLUMN user_id.
--                        Preserve idx_digests_period and idx_digests_next.
--   memory_feedback   -- drop idx_feedback_user; DROP COLUMN user_id.
--                        Preserve idx_feedback_memory.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (38);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- ============================================================================
-- 1. current_state: Shape B rebuild
-- ============================================================================

-- 1a. rename out of the way
ALTER TABLE current_state RENAME TO _current_state_old_v37;

-- 1b. drop indexes that reference the old column / table shape
DROP INDEX IF EXISTS idx_current_state_user;
DROP INDEX IF EXISTS idx_cs_key_user;
-- idx_current_state_agent and idx_cs_key are name-reused below; drop here
-- so they do not collide with the new CREATE INDEX statements.
DROP INDEX IF EXISTS idx_current_state_agent;
DROP INDEX IF EXISTS idx_cs_key;

-- 1c. create the new table without user_id
CREATE TABLE current_state (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    memory_id INTEGER REFERENCES memories(id) ON DELETE SET NULL,
    previous_value TEXT,
    previous_memory_id INTEGER,
    updated_count INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(agent, key)
);

-- 1d. copy rows forward, dropping user_id
-- On conflict (two rows with same agent+key that differed only by user_id),
-- keep the one with the higher id (most recently written value wins).
INSERT OR IGNORE INTO current_state
    (id, agent, key, value, memory_id, previous_value, previous_memory_id,
     updated_count, updated_at, created_at)
SELECT id, agent, key, value, memory_id, previous_value, previous_memory_id,
       updated_count, updated_at, created_at
FROM _current_state_old_v37
ORDER BY id DESC;

-- 1e. drop the old table
DROP TABLE _current_state_old_v37;

-- 1f. recreate indexes (agent and key COLLATE NOCASE)
CREATE INDEX IF NOT EXISTS idx_current_state_agent ON current_state(agent);
CREATE INDEX IF NOT EXISTS idx_cs_key ON current_state(key COLLATE NOCASE);

-- ============================================================================
-- 2. consolidations: Shape A DROP COLUMN
-- ============================================================================

DROP INDEX IF EXISTS idx_consolidations_user;
ALTER TABLE consolidations DROP COLUMN user_id;

-- ============================================================================
-- 3. causal_chains: Shape A DROP COLUMN
-- ============================================================================

DROP INDEX IF EXISTS idx_causal_chains_user;
ALTER TABLE causal_chains DROP COLUMN user_id;

-- ============================================================================
-- 4. reconsolidations: Shape A DROP COLUMN (no user-scoped index)
-- ============================================================================

ALTER TABLE reconsolidations DROP COLUMN user_id;

-- ============================================================================
-- 5. temporal_patterns: Shape A DROP COLUMN
-- ============================================================================

DROP INDEX IF EXISTS idx_temporal_patterns_user;
ALTER TABLE temporal_patterns DROP COLUMN user_id;

-- ============================================================================
-- 6. digests: Shape A DROP COLUMN (preserve idx_digests_period, idx_digests_next)
-- ============================================================================

DROP INDEX IF EXISTS idx_digests_user;
ALTER TABLE digests DROP COLUMN user_id;

-- ============================================================================
-- 7. memory_feedback: Shape A DROP COLUMN (preserve idx_feedback_memory)
-- ============================================================================

DROP INDEX IF EXISTS idx_feedback_user;
ALTER TABLE memory_feedback DROP COLUMN user_id;

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
