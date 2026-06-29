<!-- Thanks for contributing to Kleos! Fill in the sections below and delete this comment. -->

## What

<!-- What does this PR change, and why? One or two sentences. -->

## Related issue

<!-- e.g. "Closes #123". For significant changes, please open an issue first so we can discuss the approach. -->

## Checklist

- [ ] `scripts/preflight.sh` passes locally (add `--full` to also run the test suite)
- [ ] `cargo fmt --all -- --check` is clean
- [ ] `cargo clippy --workspace --exclude kleos-migrate --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace --exclude kleos-migrate` passes
- [ ] `cargo deny check` passes (licenses, sources, advisories, bans)
- [ ] New code has tests where applicable
- [ ] Documentation updated if behavior changes
- [ ] Commits are signed with a verified signature (see CONTRIBUTING.md, "Signed Commits")
- [ ] No unrelated changes in the PR
