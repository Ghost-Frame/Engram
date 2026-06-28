//! Centralized environment-variable lookup with `KLEOS_` primary and legacy
//! `ENGRAM_` fallback.
//!
//! The engram->kleos rename migrated crate and type names but left the
//! environment-variable namespace split: most variables were still read only
//! under `ENGRAM_`. This helper unifies the lookup so every variable resolves
//! `KLEOS_<suffix>` first and falls back to the legacy `ENGRAM_<suffix>`, which
//! keeps existing deployments working while new config uses the `KLEOS_` names.

use std::env::VarError;

/// Read an environment variable by suffix, preferring the `KLEOS_` prefix and
/// falling back to the legacy `ENGRAM_` prefix.
///
/// Pass the suffix WITHOUT a prefix: `kleos_env("GUI_PASSWORD")` reads
/// `KLEOS_GUI_PASSWORD`, then `ENGRAM_GUI_PASSWORD` if the former is unset or
/// otherwise unreadable. Returns the legacy lookup's result (success or error)
/// when the `KLEOS_` form does not resolve.
pub fn kleos_env(suffix: &str) -> Result<String, VarError> {
    // Prefer the KLEOS_ name; on any error (missing or non-unicode) fall back
    // to the legacy ENGRAM_ name so old deployments keep working.
    match std::env::var(format!("KLEOS_{suffix}")) {
        Ok(value) => Ok(value),
        Err(_) => std::env::var(format!("ENGRAM_{suffix}")),
    }
}
