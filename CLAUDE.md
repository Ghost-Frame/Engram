# Engram Rust -- Agent Instructions

## MANDATORY BUILD VERIFICATION

**Run `cargo check --workspace` after EVERY code change.** Do not move on until it passes. Do not tell the user "it should work" -- verify it compiles.

If `cargo check` fails, fix ALL errors before proceeding. Read the compiler output carefully -- Rust errors tell you exactly what's wrong.

**Run `cargo clippy --workspace` before any commit.** Fix all warnings.

---

## BEFORE WRITING ANY CODE

1. **Read the file you're modifying first.** Understand existing types, imports, and patterns.
2. **Check `lib.rs` and `mod.rs` files** to understand what's exported before writing `use` statements.
3. **Never guess at import paths.** Use the module reference below.
4. **Match existing patterns.** If surrounding code uses `&str`, don't use `String`. If it clones, clone. If it borrows, borrow.

---

## WORKSPACE STRUCTURE

```
engram-rust/
  engram-lib/     -- Core library (all domain logic)
  engram-server/  -- HTTP API server (Axum)
  engram-cli/     -- CLI client
  engram-sidecar/ -- Session-scoped memory companion
```

---

## COMMON IMPORT PATHS

These are the CORRECT import paths. Do not guess -- use these exactly.

### Database
```rust
use engram_lib::db::Database;
```

### Memory
```rust
use engram_lib::memory;
use engram_lib::memory::types::{
    Memory, StoreRequest, UpdateRequest, SearchRequest, SearchResult,
    ListOptions, SearchMode, QuestionType, MemoryCategory, MemoryStatus,
};
// Functions: memory::store, memory::get, memory::list, memory::delete,
//            memory::update, memory::mark_forgotten, memory::mark_archived
// Search:   memory::search::hybrid_search
```

### Context
```rust
use engram_lib::context::{
    assemble_context, ContextOptions, ContextStrategy, ContextMode,
    ContextBlock, ContextBlockSource, ContextBlockSummary, ContextResult,
};
```

### Config
```rust
use engram_lib::config::Config;
```

### Embeddings
```rust
use engram_lib::embeddings::EmbeddingProvider;
use engram_lib::embeddings::onnx::OnnxProvider;
```

### Reranker
```rust
use engram_lib::reranker::Reranker;
```

### Skills
```rust
use engram_lib::skills::{
    Skill, ExecutionRecord, CreateSkillRequest, UpdateSkillRequest,
    create_skill, get_skill, list_skills, update_skill, delete_skill,
    record_execution, get_executions,
};
```

### Artifacts
```rust
use engram_lib::artifacts::{
    ArtifactRow, ArtifactStats,
    store_artifact, get_artifacts_by_memory, index_artifact,
};
```

### Ingestion
```rust
use engram_lib::ingestion::{ingest, IngestOptions, IngestResult};
```

### Graph
```rust
use engram_lib::graph;  // submodules: builder, entities, communities, pagerank, search, structural, types
```

### Intelligence
```rust
use engram_lib::intelligence;  // submodules: consolidation, contradiction, decomposition, temporal, digests, reflections, sentiment, causal, extraction, growth, predictive, reconsolidation, valence
```

### Grounding
```rust
use engram_lib::grounding::types::{ToolSchema, ToolResult, BackendType, ToolStatus, SessionInfo, SessionStatus};
```

### Services
```rust
use engram_lib::services::{axon, brain, broca, chiasm, loom, soma, thymus};
```

### Errors
```rust
use engram_lib::{EngError, Result};
```

### Server-Side (engram-server only)
```rust
use crate::state::AppState;
use crate::extractors::Auth;
use crate::error::AppError;
```

---

## RUST PATTERNS USED IN THIS CODEBASE

### Ownership and Borrowing
- Database is always passed as `&Database` (shared reference)
- Config is wrapped in `Arc<Config>` in server state
- EmbeddingProvider is `Arc<dyn EmbeddingProvider>`
- Use `.clone()` on Arc types, not on the inner value
- When extracting from `Option<T>`, prefer `.as_deref()` for `Option<String>` -> `Option<&str>`

### Async
- All database operations are `async` and return `Result<T>`
- Use `.await` -- do not block with `.block_on()`
- Axum handlers are async functions returning `impl IntoResponse` or `Result<Json<T>, AppError>`

### Error Handling
- `EngError` is the library error type; `AppError` wraps it for HTTP responses
- Use `?` operator to propagate errors
- Map external errors: `.map_err(|e| EngError::Internal(e.to_string()))?`
- For not-found cases: `EngError::NotFound("description".into())`

### Serialization
- Request/response types derive `serde::Deserialize` / `serde::Serialize`
- JSON fields use snake_case
- Optional fields use `#[serde(default)]` or `Option<T>`

### Database Queries
- Uses libsql with parameterized queries
- Parameters are positional: `?1`, `?2`, etc.
- Row mapping uses index-based column access
- Always scope queries by `user_id`

### Route Handlers (engram-server)
```rust
// Standard pattern:
pub async fn handler_name(
    State(state): State<Arc<AppState>>,
    auth: Auth,
    Json(body): Json<RequestType>,
) -> Result<Json<ResponseType>, AppError> {
    // ... logic ...
    Ok(Json(response))
}
```

---

## THINGS AGENTS GET WRONG CONSTANTLY

1. **Referencing functions/types that don't exist.** Always read the module file first.
2. **Wrong import paths.** `use engram_lib::Memory` does NOT work. It's `use engram_lib::memory::types::Memory`.
3. **Forgetting to make new modules public.** Add `pub mod your_module;` to the parent `mod.rs` or `lib.rs`.
4. **Moving out of borrowed content.** Use `.clone()` or restructure to avoid moving owned values out of references.
5. **Wrong function signatures.** Read the existing function before calling it. Check the number and types of arguments.
6. **Not adding new dependencies to Cargo.toml.** If you use a new crate, add it to the workspace Cargo.toml first.
7. **Creating duplicate type definitions.** Check if the type already exists in the codebase before defining a new one.

---

## TESTING

- Run `cargo test --workspace` to run all tests
- Run `cargo test -p engram-lib` for library tests only
- Run `cargo test -p engram-server` for server tests only
- Tests use in-memory SQLite: `Database::connect_memory().await`

---

## BEHAVIOR

- **No ownership-dodging.** If you encounter an issue, take responsibility and fix it. Don't say "not caused by my changes" or "pre-existing issue." Acknowledge the problem and take initiative to resolve it. Don't give up with "known limitation" or mark things for "future work."
- **No premature stopping.** If you hit a problem, keep pushing. Don't say "good stopping point" or "natural checkpoint." Keep going until you have a complete solution.
- **No permission-seeking.** If you have the knowledge and capability to solve a problem, push through. Don't say "should I continue?" or "want me to keep going?" Take initiative and act.
- **Plan before acting.** Plan multi-step approaches before executing (which files to read, in what order, which tools to use).
- **Self-check.** Catch your own mistakes by applying reasoning loops and self-checks. Fix them before committing or asking for help.

### Tool Usage

- **Research-first, always.** Before using any tool, conduct thorough research to understand context and requirements. Never use an edit-first approach. Prefer surgical edits over rewriting whole files or large sweeping changes.
- **Use reasoning loops frequently.** Don't skip them. They ensure quality and accuracy.

### Thinking Depth

- Always apply the highest level of thinking depth for complex problem-solving. We don't mind consuming more tokens for better output.
- **Never reason from assumptions.** Reason from actual data. Read and understand the actual code, documentation, or publications before making decisions. Don't rely on guesses.

---

## STYLE

- No em dashes (--) in comments, docs, or strings. Use `--` or rewrite.
- Follow existing code style in each file
- Don't add unnecessary comments or docstrings to unchanged code
