# Slice 001: Replace the rejected all-node overview with a lossless region map and exact-memory drilldown.

- **spec:** `spec_8d420bed`

## Components

- semantic region aggregation
- region overview GPU batches
- region drilldown GPU batches
- search and fit transitions

## Hard-won conditions

- User visual acceptance remains pending
- No deployment or commit performed

## Decision: Semantic zoom with region overview

- **why:** Render aggregated real regions and inter-region edges at overview scale, then reveal original nodes and edges on region drilldown.
- **alternative:** Global force embedding -- rejected: Known 26k-node scale and edge-density problems; Confetti remains at fit scale; Exact old implementation measured 1.09 FPS
- **trust:** spec verified -- a verification run for this spec passed; this individual decision was not separately proved
