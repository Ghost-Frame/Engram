# Contributing to Engram

Thanks for your interest in contributing to Engram.

## Getting Started

```bash
git clone https://codeberg.org/GhostFrame/engram-rust.git
cd engram-rust
cargo build --workspace
cargo test --workspace
```

## Development Workflow

### Before You Code

1. **Check existing issues** - Someone might already be working on it
2. **Open an issue first** for significant changes - Lets us discuss the approach before you invest time
3. **Read CLAUDE.md** - Contains workspace structure, import paths, and patterns

### While Coding

```bash
# Verify it compiles after EVERY change
cargo check --workspace

# Run tests
cargo test --workspace

# Lint before committing
cargo clippy --workspace -- -D warnings
```

### Code Style

- Follow existing patterns in the codebase
- No em dashes in comments or docs -- use `--` or rewrite
- Prefer surgical edits over large rewrites
- Don't add features beyond what's asked
- Don't add speculative abstractions

### Commit Messages

- One logical change per commit
- Present tense, imperative mood: "Add feature" not "Added feature"
- First line under 72 characters
- Reference issues: "Fix memory leak in search (#42)"

## Pull Requests

1. Fork the repo
2. Create a feature branch: `git checkout -b feature/your-feature`
3. Make your changes
4. Run `cargo clippy --workspace -- -D warnings`
5. Run `cargo test --workspace`
6. Push and open a PR

### PR Checklist

- [ ] `cargo check --workspace` passes
- [ ] `cargo clippy --workspace` passes with no warnings
- [ ] `cargo test --workspace` passes
- [ ] New code has tests where applicable
- [ ] Documentation updated if behavior changes
- [ ] No unrelated changes in the PR

## Architecture Overview

```
engram-rust/
  engram-lib/      -- Core library (all domain logic)
  engram-server/   -- HTTP API (Axum)
  engram-cli/      -- CLI client
  engram-sidecar/  -- Session companion
  engram-mcp/      -- MCP server
  engram-cred/     -- Credential types
  engram-credd/    -- Credential daemon
  engram-approval-tui/  -- Approval TUI
  engram-migrate/  -- Migration ETL
  agent-forge/     -- Agent protocol
```

Library code goes in `engram-lib`. Server routes go in `engram-server`. Don't put domain logic in the server crate.

## Testing

- Tests use in-memory SQLite: `Database::connect_memory().await`
- Each test gets isolated state
- Use `#[tokio::test]` for async tests
- Name tests descriptively: `test_search_returns_recent_memories_first`

```rust
#[tokio::test]
async fn test_your_feature() {
    let db = Database::connect_memory().await.unwrap();
    // ... test logic
}
```

## Common Pitfalls

These trip up most contributors:

1. **Wrong import paths** - Check `engram-lib/src/lib.rs` for what's exported
2. **Forgetting `pub mod`** - New modules must be declared in parent `mod.rs`
3. **Moving out of borrows** - Use `.clone()` or restructure
4. **Missing Cargo.toml deps** - Add workspace deps before using them
5. **Not running cargo check** - Rust errors tell you exactly what's wrong

## Questions?

- Open a discussion on the repo
- Check existing issues and PRs

## License

By contributing, you agree that your contributions will be licensed under the Elastic License 2.0.
