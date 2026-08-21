# Slice 001: Replace the flat service-card GUI with a cohesive operator shell, decision-oriented Mission Control, and combined action/event Stream while preserving legacy URLs.

- **spec:** `spec_75c625c5`

## Components

- gui/src/app/AppShell.tsx
- gui/src/app/app.css
- gui/src/design/tokens.css
- gui/src/routes/Overview.tsx
- gui/src/routes/Stream.tsx
- gui/src/lib/display.ts
- gui/src/lib/services.ts
- gui/src/App.tsx

## Hard-won conditions

- Legacy /broca and /axon browser routes redirect to /stream
- Navigation is grouped by operator intent
- No literal legacy product name is introduced in user-facing React output
- npm run check exits 0

## Decision: Static topology atlas plus operator-mode application shell

- **why:** Render a bounded deterministic 2D canvas with explicit pan/zoom/search/focus and no idle animation; reorganize the whole dashboard around Mission Control, Stream, Work, Agents, Workflows, Quality, Memory, and Graph.
- **alternative:** Adopt a third-party 2D force graph and reskin all pages -- rejected: Still simulates thousands of nodes; Idle animation and unpredictable layout remain; Dependency behavior constrains accessibility and interaction
- **trust:** not independently verified
