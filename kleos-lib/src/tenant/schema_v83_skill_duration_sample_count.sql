-- Tenant schema v83: add skill_records.duration_sample_count (mirror of global
-- migration 100). Denominator for the avg_duration_ms running average so
-- executions that report no duration stop skewing the average ([51]). Backfill
-- with execution_count where an average already exists -- the closest available
-- approximation of how many samples produced it.
ALTER TABLE skill_records ADD COLUMN duration_sample_count INTEGER NOT NULL DEFAULT 0;
UPDATE skill_records SET duration_sample_count = execution_count WHERE avg_duration_ms IS NOT NULL;
