# Contributing to Kleos (formerly Engram)

## Getting Started

```bash
git clone https://github.com/Ghost-Frame/Kleos.git
cd Kleos
cargo build --workspace
cargo test --workspace
```

Rust 1.94 or later. The workspace builds on Linux, macOS, and WSL2. Native Windows builds are untested -- use WSL2.

### System Dependencies

**Debian/Ubuntu:**
```bash
sudo apt install build-essential pkg-config libssl-dev clang protobuf-compiler
```

**macOS:**
```bash
brew install openssl protobuf
```

SQLite is vendored via `rusqlite` (bundled feature). SQLCipher is vendored at compile time -- no system libsqlcipher needed.

## Workspace Structure

```
kleos/
  kleos-lib/            Core library -- all domain logic lives here
  kleos-server/         HTTP API server (Axum)
  kleos-cli/            CLI client over the HTTP API
  kleos-sidecar/        Session-scoped memory proxy
  kleos-mcp/            MCP server (Model Context Protocol)
  kleos-cred/           Credential management library
  kleos-credd/          Credential management daemon
  kleos-approval-tui/   Approval workflow TUI (WIP)
  kleos-migrate/        libsql -> rusqlite ETL tool
  agent-forge/          Structured reasoning CLI
  sdk/                  Client SDKs (TypeScript)
  hooks/                Claude Code hook scripts
```

**Key rule:** domain logic goes in `kleos-lib`. Server routes go in `kleos-server`. Don't put business logic in the server crate.

## Development Workflow

### Before You Code

1. Check existing issues - someone might already be working on it
2. Open an issue first for significant changes - lets us discuss the approach before you invest time

### While Coding

```bash
# Verify it compiles after every change
cargo check --workspace

# Run tests (in-memory SQLite, no external deps)
cargo test --workspace

# Lint before committing - warnings are errors
cargo clippy --workspace - -D warnings
```

### Code Style

- Follow existing patterns in the codebase
- Prefer surgical edits over large rewrites
- Don't add features beyond what's asked
- Don't add speculative abstractions
- New modules must be declared in the parent `mod.rs` with `pub mod`
- DTOs and request/response types go in `types.rs` within each module

### Commit Messages

- One logical change per commit
- Present tense, imperative mood: "add feature" not "added feature"
- First line under 72 characters
- Reference issues when applicable: "fix memory leak in search (#42)"

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

## Testing

Tests use in-memory SQLite - no database setup needed. Each test gets isolated state.

```rust
#[tokio::test]
async fn test_search_returns_recent_memories_first() {
    let db = Database::connect_memory().await.unwrap();
    // ... test logic
}
```

- Use `#[tokio::test]` for async tests
- Name tests descriptively: `test_search_returns_recent_memories_first`
- Tests run with `cargo test --workspace` -- no feature flags needed

## Common Pitfalls

1. **Wrong import paths** - check `kleos-lib/src/lib.rs` for what's exported
2. **Forgetting `pub mod`** - new modules must be declared in parent `mod.rs`
3. **Moving out of borrows** - use `.clone()` or restructure; the compiler tells you exactly what's wrong
4. **Missing Cargo.toml deps** - add workspace deps before using them
5. **Not running cargo check** - Rust errors are precise; read them before asking for help
6. **Feature-gated code** - the `brain` module requires `brain_hopfield` feature flag

## Feature Flags

| Crate | Flag | Default | Purpose |
|-------|------|---------|---------|
| `kleos-lib` | `brain_hopfield` | on | Brain module, Hopfield networks, spreading activation |
| `kleos-lib` | `sqlcipher` | off | SQLCipher at-rest encryption (required for `KLEOS_ENCRYPTION_MODE` != `none`) |
| `kleos-lib` | `bundled-sqlite` | off | Vendor SQLite from source (needed on Windows) |
| `kleos-lib` | `tenant-sharding` | off | Per-tenant database sharding |
| `kleos-lib` | `test-utils` | off | Test helpers for downstream crates |
| `kleos-lib` | `credd-raw` | off | Raw credential access support |
| `kleos-mcp` | `http` | off | HTTP transport (default is stdio only) |
| `kleos-cred` | `gui` | off | eframe-based credential manager GUI |

`sqlcipher` and `bundled-sqlite` are mutually exclusive with `libsql` in the same binary (symbol collision). This is why `kleos-migrate` cannot enable `sqlcipher`.

## Migration Tool

`kleos-migrate` is a one-shot ETL utility for migrating data from the old libsql-backed database to the current rusqlite + LanceDB backend. It reads the source database, transforms records, and writes them to the new format. Run it once during migration -- it is not part of normal operation.

```bash
cargo run -p kleos-migrate -- --source old-engram.db --target /data/kleos
```

The `--target` is a directory - rusqlite database and LanceDB index are created inside it. Use `--dry-run` to preview table counts and schema diffs without writing. Use `--skip-vectors` to copy relational data only.

Note: `kleos-migrate` has a linker conflict between libsqlite3-sys and libsql-ffi. It must be built separately and cannot share a binary with the server.

## Questions?

- Open a discussion on the repo
- Check existing issues and PRs

## License

By contributing, you agree that your contributions will be licensed under the Elastic License 2.0.
