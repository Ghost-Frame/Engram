# hooks

The Kleos hooks bundle is **under maintenance** and not currently shipped.

Prior versions of this directory contained example Claude Code session hooks
(`session-start`, `user-prompt`, `session-end`, etc.) that wired the CLI into
Kleos at agent session boundaries. Those scripts have been removed pending a
rewrite that decouples them from operator-specific tooling.

If you need session-start integration today, call `kleos-cli hook session-start`
directly from your own hook script. The CLI subcommand is supported and
documented in `docs/KLEOS_OPERATIONS_MANUAL.md`. The mandatory-rules text is
operator-configurable via the `KLEOS_MANDATORY_RULES` environment variable on
the server -- see `kleos-server/src/routes/policy/mod.rs`.

A new hooks bundle will be reintroduced once the surface stabilises.
