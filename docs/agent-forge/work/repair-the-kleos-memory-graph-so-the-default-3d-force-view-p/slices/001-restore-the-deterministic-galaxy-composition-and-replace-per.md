# Slice 001: Restore the deterministic galaxy composition and replace per-node/per-edge draw calls with interactive GPU batches.

- **spec:** `spec_50e144c5`

## Components

- gui/src/routes/memory/Graph.tsx
- gui/src/routes/memory/graph.css
- gui/src/lib/api/graph.ts
- gui/src/routes/memory/Graph.test.tsx
- gui/src/lib/api/clients.test.ts

## Hard-won conditions

- No production deployment
- Search, selection, hover, zoom, filtering, reduced motion, and accessibility remain functional
- Selecting top-ranked graph nodes before edge expansion removes lower-ranked bridge nodes and can manufacture disconnected components. Live evidence: 47 of 48 capped minor components were entirely within the uncapped giant component. (Kleos graph builder and GUI connectivity debugging)

## Decision: Connected sampler plus batched interactive galaxy

- **why:** Add an opt-in rank-aware connected server sample, restore deterministic galaxy targets, render nodes and links as batched GPU buffers, and provide explicit point picking and search/detail fallbacks.
- **alternative:** Connected server sampler with existing renderer -- rejected: Measured renderer remains near 10 FPS; Thousands of independent draw calls keep interaction laggy
- **trust:** not independently verified
