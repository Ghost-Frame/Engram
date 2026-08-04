//! Filesystem layout for emitted documentation. Every emitted path is derived
//! here so the layout lives in exactly one place.

use std::path::{Path, PathBuf};

/// Directory, relative to the repository root, holding all emitted documentation.
pub const DOC_ROOT: &str = "docs/agent-forge";

/// Maximum slug length, chosen to keep emitted filenames comfortably short.
const MAX_SLUG_LEN: usize = 60;

/// Convert free text into a stable, filesystem-safe slug. Runs of non-alphanumeric
/// characters collapse to a single dash; the result is lowercase, has no leading or
/// trailing dash, and is capped at `MAX_SLUG_LEN`. Text with no alphanumeric content
/// yields "untitled" so the caller always gets a usable filename.
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
                // Re-check the cap immediately: a dash push and the letter push
                // that follows both happen within this single loop iteration, so
                // checking only once per iteration (below) could let the pair
                // overshoot MAX_SLUG_LEN by one character.
                if out.len() >= MAX_SLUG_LEN {
                    break;
                }
            }
            pending_dash = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
        if out.len() >= MAX_SLUG_LEN {
            break;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Absolute path to the emitted-documentation root inside `repo_root`.
pub fn doc_root(repo_root: &Path) -> PathBuf {
    repo_root.join(DOC_ROOT)
}

/// Directory holding one spec's record and slice documents.
pub fn spec_dir(repo_root: &Path, slug: &str) -> PathBuf {
    doc_root(repo_root).join("work").join(slug)
}

/// Path to a spec's top-level record document.
pub fn record_path(repo_root: &Path, slug: &str) -> PathBuf {
    spec_dir(repo_root, slug).join("record.md")
}

/// Path to a spec's human-facing requirements document.
pub fn requirements_path(repo_root: &Path, slug: &str) -> PathBuf {
    spec_dir(repo_root, slug).join("requirements.md")
}

/// Path to a spec's human-facing design document.
pub fn spec_design_path(repo_root: &Path, slug: &str) -> PathBuf {
    spec_dir(repo_root, slug).join("design.md")
}

/// Path to a spec's evidence-derived task document.
pub fn tasks_path(repo_root: &Path, slug: &str) -> PathBuf {
    spec_dir(repo_root, slug).join("tasks.md")
}

/// Directory holding a spec's per-checkpoint slice documents.
pub fn slices_dir(repo_root: &Path, slug: &str) -> PathBuf {
    spec_dir(repo_root, slug).join("slices")
}

/// Path to one numbered slice document, zero-padded so lexical order matches
/// chronological order.
pub fn slice_path(repo_root: &Path, slug: &str, index: i64, slice_slug: &str) -> PathBuf {
    slices_dir(repo_root, slug).join(format!("{:03}-{}.md", index, slice_slug))
}

/// Path to a spec's design document.
pub fn design_path(repo_root: &Path, slug: &str) -> PathBuf {
    doc_root(repo_root)
        .join("design")
        .join(format!("{}.md", slug))
}

#[cfg(test)]
/// Tests for slug generation and path construction.
mod tests {
    use super::*;
    use std::path::Path;

    /// A plain description becomes a lowercase, dash-separated slug.
    #[test]
    fn slugify_lowercases_and_dashes() {
        assert_eq!(slugify("Add Emission Layer"), "add-emission-layer");
    }

    /// Punctuation collapses into single dashes with no leading or trailing dash.
    #[test]
    fn slugify_collapses_punctuation() {
        assert_eq!(slugify("  fix: the //thing!!  "), "fix-the-thing");
    }

    /// Slugs are capped so they cannot produce unusable filenames.
    #[test]
    fn slugify_caps_length_without_trailing_dash() {
        let s = slugify(&"word ".repeat(40));
        assert!(s.len() <= 60);
        assert!(!s.ends_with('-'));
    }

    /// A description with no alphanumeric content still yields a usable slug.
    #[test]
    fn slugify_handles_empty_result() {
        assert_eq!(slugify("!!!"), "untitled");
    }

    /// Paths nest under the documentation root in the layout the spec defines.
    #[test]
    fn paths_follow_the_documented_layout() {
        let root = Path::new("/repo");
        assert_eq!(
            record_path(root, "my-spec"),
            Path::new("/repo/docs/agent-forge/work/my-spec/record.md")
        );
        assert_eq!(
            requirements_path(root, "my-spec"),
            Path::new("/repo/docs/agent-forge/work/my-spec/requirements.md")
        );
        assert_eq!(
            spec_design_path(root, "my-spec"),
            Path::new("/repo/docs/agent-forge/work/my-spec/design.md")
        );
        assert_eq!(
            tasks_path(root, "my-spec"),
            Path::new("/repo/docs/agent-forge/work/my-spec/tasks.md")
        );
        assert_eq!(
            slice_path(root, "my-spec", 2, "wire-it"),
            Path::new("/repo/docs/agent-forge/work/my-spec/slices/002-wire-it.md")
        );
        assert_eq!(
            design_path(root, "my-spec"),
            Path::new("/repo/docs/agent-forge/design/my-spec.md")
        );
    }
}
