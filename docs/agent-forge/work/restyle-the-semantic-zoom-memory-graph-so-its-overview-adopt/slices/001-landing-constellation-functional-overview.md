# Slice 001: landing-constellation-functional-overview

- **spec:** `spec_15c99649`

## Components

- semantic region aggregation
- GPU region rendering
- landing-style graph chrome
- large-graph regression coverage

## Hard-won conditions

- 26,833-memory fixture loads
- region click enters detail
- Fit Galaxy returns to overview
- production build passes

## Decision: Restyle semantic zoom in place

- **why:** Keep the lossless region atlas and drilldown behavior; change region materials, edge treatment, backdrop, and chrome to the landing visual system.
- **alternative:** Replace overview with the landing Canvas2D demo -- rejected: Wallpaper-only demo; Does not represent the real dataset; Duplicates render architecture
- **trust:** not independently verified
