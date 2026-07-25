# Slice 002: Make the Kleos GUI coherent, useful, and responsive while preserving legacy cookie compatibility.

- **spec:** `spec_75c625c5`

## Components

- operator shell and navigation
- mission control overview
- unified signal stream
- Kleos display-name normalization
- bounded 2D Memory Atlas
- canonical GUI auth cookie compatibility

## Hard-won conditions

- frontend type check passes
- all frontend tests pass
- production frontend build passes
- backend compatibility tests pending final cargo invocation

## Decision: Static topology atlas plus operator-mode application shell

- **why:** Render a bounded deterministic 2D canvas with explicit pan/zoom/search/focus and no idle animation; reorganize the whole dashboard around Mission Control, Stream, Work, Agents, Workflows, Quality, Memory, and Graph.
- **alternative:** Adopt a third-party 2D force graph and reskin all pages -- rejected: Still simulates thousands of nodes; Idle animation and unpredictable layout remain; Dependency behavior constrains accessibility and interaction
- **trust:** not independently verified
