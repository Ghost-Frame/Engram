-- Tenant schema v36: drop user_id from the thymus cluster (5 tenant-side tables).
--
-- Tables handled and their shapes:
--   rubrics              -- Shape A with index swap: drop idx_rubrics_user_name +
--                           idx_rubrics_user; DROP COLUMN user_id; create new
--                           UNIQUE INDEX idx_rubrics_name ON rubrics(name).
--                           No table rebuild needed because uniqueness was
--                           enforced by a separate UNIQUE INDEX, not an in-table
--                           UNIQUE constraint.
--   evaluations          -- Shape A: drop idx_evaluations_user; DROP COLUMN user_id.
--                           The rubric_id FK to rubrics(id) is unaffected.
--   quality_metrics      -- Shape A: drop idx_quality_metrics_user; DROP COLUMN user_id.
--   session_quality      -- Shape A: drop idx_session_quality_user; DROP COLUMN user_id.
--   behavioral_drift_events -- Shape A: drop idx_behavioral_drift_user; DROP COLUMN user_id.
--
-- PRAGMA wrap is included defensively for the rubrics index swap even though
-- no table rebuild is required; this matches the Stage 6 convention.

INSERT OR IGNORE INTO schema_migrations (version) VALUES (36);

PRAGMA foreign_keys = OFF;
PRAGMA legacy_alter_table = 1;

-- ============================================================================
-- 1. rubrics: Shape A with index swap
-- ============================================================================

-- Drop the old user-scoped unique index and plain user index.
DROP INDEX IF EXISTS idx_rubrics_user_name;
DROP INDEX IF EXISTS idx_rubrics_user;

-- Drop the user_id column.
ALTER TABLE rubrics DROP COLUMN user_id;

-- Recreate uniqueness on name alone (single-tenant; equivalent constraint).
CREATE UNIQUE INDEX IF NOT EXISTS idx_rubrics_name ON rubrics(name);

-- ============================================================================
-- 2. evaluations: Shape A
-- ============================================================================

DROP INDEX IF EXISTS idx_evaluations_user;
ALTER TABLE evaluations DROP COLUMN user_id;

-- ============================================================================
-- 3. quality_metrics: Shape A
-- ============================================================================

DROP INDEX IF EXISTS idx_quality_metrics_user;
ALTER TABLE quality_metrics DROP COLUMN user_id;

-- ============================================================================
-- 4. session_quality: Shape A
-- ============================================================================

DROP INDEX IF EXISTS idx_session_quality_user;
ALTER TABLE session_quality DROP COLUMN user_id;

-- ============================================================================
-- 5. behavioral_drift_events: Shape A
-- ============================================================================

DROP INDEX IF EXISTS idx_behavioral_drift_user;
ALTER TABLE behavioral_drift_events DROP COLUMN user_id;

PRAGMA legacy_alter_table = 0;
PRAGMA foreign_keys = ON;
