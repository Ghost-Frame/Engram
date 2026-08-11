# Slice 001: Prevent Dreamer from starving live memory stores on large tenants.

- **spec:** `spec_29b1c35e`

## Components

- discover.rs caps similarity comparisons, permutes pattern order, preloads tenant association edges once, and canonicalizes pair direction.
- dream/tests.rs verifies the comparison cap, zero-budget behavior, and reverse-edge deduplication.

## Hard-won conditions

- Production budget 64 permits at most 4,096 pair comparisons.
- Existing association preload remains scoped by user_id and edge_type.
- All 17 focused dream tests pass.
- Kleos dream discovery's edge budget did not bound pair comparisons. Large tenants require a separate comparison cap, one scoped preload of existing associations, and canonical pair keys when scan order changes.

## Decision: Bounded permuted scan with edge preload

- **why:** Randomize pattern order per cycle, cap comparisons at budget times a fixed multiplier, and preload all tenant association pairs into a canonical HashSet.
- **alternative:** Approximate nearest-neighbor discovery -- rejected: Large architectural change; Index lifecycle and consistency work; Too risky for an urgent production repair
- **trust:** spec verified -- a verification run for this spec passed; this individual decision was not separately proved
