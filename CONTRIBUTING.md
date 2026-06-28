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
  kleos-migrate/        Monolith-to-tenant shard ETL tool
  agent-forge/          Structured reasoning CLI
  sdk/                  Client SDKs (TypeScript)
  hooks/                Claude Code hook scripts
```

**Key rule:** domain logic goes in `kleos-lib`. Server routes go in `kleos-server`. Don't put business logic in the server crate.

## Development Workflow

### Before You Code

1. Check existing issues - someone might already be working on it
2. Open an issue first for significant changes - lets us discuss the approach before you invest time

### Setup (once per clone)

```bash
# Install the tracked git hooks: rustfmt-on-commit, and a pre-push gate that
# runs the CI checks locally so a 25-minute CI cycle never fails on a lint.
scripts/install-git-hooks.sh
```

### While Coding

```bash
# Verify it compiles after every change
cargo check --workspace

# Run tests (in-memory SQLite, no external deps)
cargo test --workspace

# Lint before committing - warnings are errors
cargo clippy --workspace -- -D warnings
```

### Before Pushing

Run the exact CI gate locally -- this is what `ci.yml` runs, and what the
pre-push hook runs for you:

```bash
# Fast gate (fmt + clippy + cargo-deny + MSRV) -- a couple of minutes:
scripts/preflight.sh

# Full gate (also cargo check + the workspace test suite):
scripts/preflight.sh --full
```

The individual CI commands, if you prefer to run them by hand:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --exclude kleos-migrate --all-targets -- -D warnings
cargo test --workspace --exclude kleos-migrate
cargo deny check        # licenses, sources, advisories, bans -- fails independently of your code
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
4. Run `scripts/preflight.sh` (or `--full` to include tests)
5. Push and open a PR

### PR Checklist

- [ ] `cargo fmt --all -- --check` is clean
- [ ] `cargo clippy --workspace --exclude kleos-migrate --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace --exclude kleos-migrate` passes
- [ ] `cargo deny check` passes (licenses, sources, advisories, bans)
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

## Migration Tool

`kleos-migrate` copies data from an encrypted SQLCipher monolith database into a per-tenant shard. It filters rows by `--filter-user-id` and writes them to a new tenant directory with `kleos.db` and LanceDB vector index. One-shot utility, not part of normal operation.

```bash
cargo run -p kleos-migrate -- --source system.db --target /data/tenants/1 --filter-user-id 1
```

Use `--dry-run` to preview per-table counts without writing. Use `--handoffs-source` to migrate a legacy handoffs database into the target shard.

## Questions?

- Open a discussion on the repo
- Check existing issues and PRs

## License

By contributing, you agree that your contributions will be licensed under the Elastic License 2.0.
