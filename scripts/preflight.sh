#!/usr/bin/env bash
# preflight.sh -- run the project's CI gate locally before pushing.
#
# A failed push costs a ~25-minute CI cycle (the workspace statically links the
# full lance/arrow/ort/tantivy tree into every test binary). Most failures are
# cheap to catch: clippy lints, rustfmt drift, and cargo-deny policy. This
# script runs those in a couple of minutes so a long CI cycle never fails on
# something a local check would have caught. It mirrors .github/workflows/ci.yml
# exactly -- keep the two in sync.
#
# Usage:
#   scripts/preflight.sh         # fast gate: fmt + clippy + deny + MSRV (no tests)
#   scripts/preflight.sh --full  # also run cargo check + the full test suite
#
# Exits non-zero if any gate fails.

set -uo pipefail
cd "$(git rev-parse --show-toplevel)"

# Whether to also run the slow steps (check + test). Off by default so the
# pre-push hook stays fast; tests are still enforced by CI.
FULL=0
[[ "${1:-}" == "--full" ]] && FULL=1

fail=0

# Print a step header.
step() { printf '\n==> %s\n' "$*"; }

# Run a command, mark the run failed (but keep going) if it exits non-zero, so
# the operator sees every problem in one pass instead of one at a time.
run() {
    if "$@"; then
        echo "   OK"
    else
        echo "   FAILED: $*"
        fail=1
    fi
}

# rustfmt -- CI: cargo fmt --all -- --check
step "rustfmt --check"
run cargo fmt --all -- --check

# clippy -- CI: cargo clippy --workspace --exclude kleos-migrate --all-targets -- -D warnings
step "clippy (workspace, all targets, -D warnings)"
run cargo clippy --workspace --exclude kleos-migrate --all-targets -- -D warnings

# MSRV declaration -- CI greps Cargo.toml for the pinned rust-version.
step "MSRV declaration (rust-version = 1.94)"
if grep -q 'rust-version = "1.94"' Cargo.toml; then
    echo "   OK"
else
    echo "   FAILED: workspace rust-version not declared or drifted"
    fail=1
fi

# cargo-deny -- CI: cargo deny check (licenses, sources, advisories, bans).
# This is the gate that fails independently of your code (e.g. a versionless
# path dep is a wildcard, or a freshly published advisory). Run it if the tool
# is present; warn loudly otherwise, because CI will run it regardless.
step "cargo deny check"
if command -v cargo-deny >/dev/null 2>&1; then
    run cargo deny check
else
    echo "   SKIPPED: cargo-deny not installed -- CI WILL still run it."
    echo "   Install once with: cargo install --locked cargo-deny"
fi

# Slow steps, opt-in via --full.
if [[ "$FULL" == "1" ]]; then
    step "cargo check --workspace"
    run cargo check --workspace

    # Drop test-profile debuginfo like CI so the link inputs stay small.
    step "cargo test --workspace (excluding kleos-migrate)"
    if CARGO_PROFILE_TEST_DEBUG=0 cargo test --workspace --exclude kleos-migrate; then
        echo "   OK"
    else
        echo "   FAILED: cargo test"
        fail=1
    fi
fi

echo
if [[ "$fail" == "0" ]]; then
    echo "preflight: ALL GREEN -- safe to push."
else
    echo "preflight: FAILURES above -- fix before pushing (bypass: PREFLIGHT_SKIP=1 git push)."
fi
exit "$fail"
