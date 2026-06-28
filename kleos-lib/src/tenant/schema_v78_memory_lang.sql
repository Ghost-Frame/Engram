-- Tenant schema v78: add a nullable `lang` column to the shard `memories` table
-- for the detected ISO 639-1 content language (mirror of global migration 95).
-- Nullable, no backfill -- NULL means "unknown / treat as en". Populated by
-- detect_lang on subsequent writes.
ALTER TABLE memories ADD COLUMN lang TEXT;
