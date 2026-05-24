-- Tenant schema v67: re-add user_id to structured_facts and
-- entity_cooccurrences. Both were dropped by v35, never re-added on the
-- tenant side.
--
-- Simple ADD COLUMN for both tables. Idempotency relies on the
-- schema_migrations version gate in the tenant migration runner (this file
-- only executes if version < 67).

INSERT OR IGNORE INTO schema_migrations (version) VALUES (67);

ALTER TABLE structured_facts ADD COLUMN user_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE entity_cooccurrences ADD COLUMN user_id INTEGER DEFAULT 1;

CREATE INDEX IF NOT EXISTS idx_sf_user ON structured_facts(user_id);
CREATE INDEX IF NOT EXISTS idx_ec_user ON entity_cooccurrences(user_id);
