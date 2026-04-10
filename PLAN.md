# Task: Composite Index for Contradiction Detection

**Branch:** feat/scale-index
**Effort:** 1 hour
**Model:** Haiku (trivial)

## Goal

Add composite index on structured_facts(user_id, subject, predicate) to fix O(n^2) contradiction scan.

## File

- Modify: `engram-lib/src/db/schema.rs`

## Steps

- [ ] 1. Find the structured_facts indexes section (around line 410-420)

- [ ] 2. Add new index after existing ones:
```sql
CREATE INDEX IF NOT EXISTS idx_facts_user_subject_predicate
  ON structured_facts(user_id, subject, predicate);
```

- [ ] 3. Run: `cargo test -p engram-lib`

- [ ] 4. Verify with EXPLAIN QUERY PLAN (manual check):
```sql
EXPLAIN QUERY PLAN
SELECT sf1.memory_id, sf2.memory_id
FROM structured_facts sf1
JOIN structured_facts sf2
  ON sf1.subject = sf2.subject
  AND sf1.predicate = sf2.predicate
  AND sf1.id < sf2.id
WHERE sf1.user_id = 1;
```
Should show: SEARCH ... USING INDEX idx_facts_user_subject_predicate

- [ ] 5. Commit: `feat(db): add composite index for contradiction detection`

## Done When

- Index exists in schema
- Tests pass
- Commit made
