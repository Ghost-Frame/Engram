# Worktree H: Context Assembly Completion

## Worktree Path
`/home/zan/Projects/engram-rust-wt-H` -- branch `feat/context-assembly`

## Goal
Complete the 5 stubbed phases in context assembly so the Rust server produces the same context output as the TypeScript engram. This is HIGH priority -- context assembly is the core value proposition.

## Source of Truth
- `C:\Users\Zan\Projects\engram\src\context\` -- TS context assembly (this is the authority)
- `C:\Users\Zan\Projects\engram-rust\engram-lib\src\context\` -- current Rust implementation

## Files to Modify
- `engram-lib/src/context/mod.rs` -- main assembly pipeline (5 stubbed phases)
- `engram-lib/src/context/deps.rs` -- dependency resolution
- `engram-lib/src/context/scoring.rs` -- scoring logic
- `engram-lib/src/context/budget.rs` -- token budget management
- `engram-lib/src/context/modes.rs` -- context modes
- Any other context files as needed

## Stubbed Phases (from context/mod.rs)

### 1. Embedding Map for Dedup (line ~205)
Currently: `// stubbed -- no cached embeddings available yet`
Need: Wire in `EmbeddingProvider` to compute embeddings for dedup. The provider exists in `engram-lib/src/embeddings/`. Thread it through `ContextOptions` or `ContextDeps`.

### 2. Query Embedding (line ~215)
Currently: `// stubbed -- no embedding provider wired in yet`
Need: Embed the query text using the same provider. Used for semantic similarity scoring.

### 3. Inference / LLM Connections (line ~510)
Currently: `// stubbed -- no LLM available yet`
Need: Use `LocalModelClient` from `engram-lib/src/llm/` to generate implicit connections. Read the TS implementation to understand what this phase does.

### 4. Scratchpad / Working Memory (line ~517)
Currently: `// stubbed -- scratchpad module not yet available`
Need: Wire in `engram-lib/src/scratchpad.rs`. Read the module to understand what it provides, then inject it into context.

### 5. Personality Profile (line ~539)
Currently: `// stubbed -- personality module not fully implemented`
Need: Wire in `engram-lib/src/personality.rs`. Read the module, then inject personality data into assembled context.

## Approach
1. Read the ENTIRE TS context directory first. Understand the full pipeline.
2. Read the current Rust context/mod.rs to see how phases are structured.
3. Read each dependency module (embeddings, llm, scratchpad, personality) to understand their APIs.
4. Implement phases in order (1-5).
5. After each phase, run `cargo check --workspace`.

## Constraints
- The `ContextOptions` and `ContextResult` types must not change their public shape
- The embedding provider is `Arc<dyn EmbeddingProvider>` -- thread it through deps
- Match TS behavior, don't invent new logic
- Run `cargo check --workspace` after every change
- Run `cargo clippy --workspace` before committing
- No em dashes

## Verification
1. `cargo check --workspace` passes
2. `cargo test -p engram-lib context` passes
3. No more "stubbed" comments in context/mod.rs
4. `grep -n "stubbed" engram-lib/src/context/mod.rs` returns nothing
