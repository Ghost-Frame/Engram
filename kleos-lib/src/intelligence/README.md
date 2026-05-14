# intelligence

Multi-pipeline knowledge processing: fact extraction, embedding, emotional valence, temporal
analysis, and scheduled consolidation jobs that run against the `memories` table.

## Key types (`types.rs`)

| Type | Purpose |
|------|---------|
| `IntelligenceTier` | `Auto / Llm / Rules / Template` -- selects processing path via `KLEOS_INTELLIGENCE_TIER` env var (legacy `ENGRAM_INTELLIGENCE_TIER` accepted as fallback) |
| `DecompositionResult` / `DecompositionWithTier` | Output of fact decomposition; carries which tier ran |
| `FactStoreMeta` | Caller-supplied context for routing a fact into the right memory slot |
| `ValenceResult` / `EmotionMatch` | Emotional valence + arousal scores from lexicon analysis |
| `ReconsolidationResult` | Before/after importance and confidence after a reconsolidation pass |
| `CausalChain` / `CausalLink` | Linked cause-effect chains stored in `causal_chains` table |
| `PipelineReport` / `TaskReport` | Structured result from the scheduler pipeline |
| `Contradiction` | A detected conflict between two memory candidates |
| `MemoryHealthReport` | Coverage stats: embeddings, archives, superseded rows |

## Module map

| File | Role |
|------|------|
| `extraction.rs` | Regex-based extraction of facts, preferences, and state (no LLM required) |
| `decomposition.rs` | Tiered decomposition: LLM -> rule-based -> template fallback |
| `valence.rs` | Lexicon-driven valence + arousal scoring; writes to `memories.valence` / `arousal` |
| `sentiment.rs` | Lightweight sentiment signals feeding valence |
| `consolidation.rs` | Merges near-duplicate memory pairs into summaries |
| `duplicates.rs` | SimHash-based duplicate sweeper; reports `DuplicatePair` |
| `contradiction.rs` | Cross-memory contradiction detection |
| `correction.rs` | Applies corrections and updates confidence on conflicting facts |
| `reconsolidation.rs` | Re-evaluates importance/confidence for existing memories |
| `reflections.rs` | Generates periodic `Reflection` summaries keyed to `ReflectionPeriod` |
| `causal.rs` | Mines and walks `CausalChain` links |
| `temporal.rs` | Temporal pattern mining and time-travel queries |
| `predictive.rs` | Predicts categories/projects from time context; returns `PredictiveContext` |
| `growth.rs` | Writes `GrowthObservation` entries for agent self-improvement signals |
| `feedback.rs` | Records `FeedbackRequest` ratings; updates memory quality signals |
| `digests.rs` | Produces `Digest` summaries for day/week/month periods |
| `health.rs` | Computes `MemoryHealthReport` across the store |
| `scheduler.rs` | Runs the full intelligence pipeline as a sequence of `TaskReport` steps |
| `llm.rs` | Shared LLM call wrapper used by decomposition, reflections, and growth |

## Data flow

```
Incoming text
  -> extraction.rs   (regex facts, preferences, state)
  -> decomposition.rs (tiered: llm | rules | template)
  -> valence.rs       (writes valence/arousal back to memories row)
  -> scheduler.rs     (periodic: consolidation, duplicates, reconsolidation, reflections)
```

`scheduler.rs` runs consolidation, duplicate detection, reconsolidation, and reflection
generation as a pipeline and emits a `PipelineReport`. Individual passes are also callable
directly. The `llm.rs` wrapper handles temperature and token limits via `LlmOptions`.

## Key files

- `mod.rs` -- module re-exports only; no logic
- `types.rs` -- all shared DTOs
- `scheduler.rs` -- pipeline driver entry point
- `extraction.rs` -- the fast no-LLM path for fact capture
