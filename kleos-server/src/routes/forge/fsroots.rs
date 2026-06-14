//! Filesystem root resolver for server-side agent-forge compute routes.
//!
//! Paths supplied by callers are only forwarded to `agent_forge` tools when
//! they resolve to a location strictly inside one of the operator-configured
//! roots. This prevents path-traversal attacks where a caller could point
//! the server at arbitrary filesystem locations.
//!
//! Configuration: `KLEOS_FORGE_FS_ROOTS` -- colon-separated list of absolute
//! directory paths that the server is permitted to read. If the variable is
//! unset or empty, `resolve_within_roots` returns `None` for every input.
use std::path::PathBuf;

/// Canonicalize `path` and return it only when it lies under one of the roots
/// listed in `KLEOS_FORGE_FS_ROOTS` (colon-separated). Returns `None` if:
/// - the env var is unset or empty,
/// - `path` cannot be canonicalized (does not exist, bad encoding, etc.),
/// - the canonicalized path does not start_with any allowed root.
pub fn resolve_within_roots(path: &str) -> Option<PathBuf> {
    // Read and split the allow-list.
    let roots_raw = std::env::var("KLEOS_FORGE_FS_ROOTS").ok()?;
    if roots_raw.trim().is_empty() {
        return None;
    }

    // Canonicalize the candidate path so symlinks and `..` are resolved before
    // the prefix comparison. If the path does not exist, reject immediately.
    let candidate = std::fs::canonicalize(path).ok()?;

    // Check each configured root: canonicalize the root itself so that both
    // sides of the comparison are fully resolved. Skip roots that do not exist.
    for root_str in roots_raw.split(':') {
        let root_str = root_str.trim();
        if root_str.is_empty() {
            continue;
        }
        let root = match std::fs::canonicalize(root_str) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if candidate.starts_with(&root) {
            return Some(candidate);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    /// Unit tests for `resolve_within_roots`. Each test manipulates the env
    /// variable directly; they must not run in parallel (`--test-threads=1`).
    use super::*;
    use std::env;
    use tempfile::TempDir;

    /// Helper: create a real temp directory whose canonical form we can use.
    fn make_tempdir() -> TempDir {
        tempfile::tempdir().expect("tempdir creation must succeed")
    }

    /// A path that is a direct child of a configured root is allowed.
    #[test]
    fn within_root_returns_some() {
        let dir = make_tempdir();
        let root = dir.path().to_str().expect("tempdir must be valid UTF-8");

        // Create a real file inside the root so canonicalize succeeds.
        let child = dir.path().join("file.txt");
        std::fs::write(&child, b"x").unwrap();

        env::set_var("KLEOS_FORGE_FS_ROOTS", root);
        let result = resolve_within_roots(child.to_str().unwrap());
        env::remove_var("KLEOS_FORGE_FS_ROOTS");

        assert!(result.is_some(), "child of configured root must resolve");
    }

    /// A path outside every configured root must be rejected.
    #[test]
    fn outside_root_returns_none() {
        let allowed = make_tempdir();
        let other = make_tempdir();

        // Create a real file in `other` so canonicalize does not fail.
        let child = other.path().join("file.txt");
        std::fs::write(&child, b"x").unwrap();

        let root = allowed.path().to_str().unwrap();
        env::set_var("KLEOS_FORGE_FS_ROOTS", root);
        let result = resolve_within_roots(child.to_str().unwrap());
        env::remove_var("KLEOS_FORGE_FS_ROOTS");

        assert!(result.is_none(), "path outside roots must be rejected");
    }

    /// When the env var is absent the function must always return None.
    #[test]
    fn no_env_returns_none() {
        let dir = make_tempdir();
        let child = dir.path().join("file.txt");
        std::fs::write(&child, b"x").unwrap();

        env::remove_var("KLEOS_FORGE_FS_ROOTS");
        let result = resolve_within_roots(child.to_str().unwrap());

        assert!(result.is_none(), "absent env var must yield None");
    }
}
