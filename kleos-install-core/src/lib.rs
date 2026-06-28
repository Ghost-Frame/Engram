//! `kleos-install-core` -- shared core library for the Kleos interactive installer.
//!
//! Contains all non-UI logic used by both the TUI (`ratatui`) and GUI (`egui`)
//! installer frontends. All modules are public so that frontend crates can
//! import individual types directly without going through this crate's re-exports.

/// Component registry, platform definitions, and installation profiles.
pub mod components;

/// Error type shared across all installer operations.
pub mod error;

/// Platform detection and default path resolution.
pub mod platform;

/// Binary download, checksum verification, and GitHub release API.
pub mod download;

/// Configuration types and file generation (kleos.toml + .env).
pub mod config;

/// Cryptographic key generation for secrets and API keys.
pub mod security;

/// System service integration (systemd, launchd).
pub mod system;

/// Upgrade detection and config migration from existing installations.
pub mod upgrade;

/// Full installation plan and execution orchestration.
pub mod plan;

// Convenience re-exports for the most commonly used types.
pub use components::{
    all_components, profile_components, resolve_dependencies, Component, Platform, Profile,
};
pub use config::{EmbeddingConfig, InstallerConfig, RerankerConfig, SecurityConfig, ServerConfig};
pub use download::{DownloadProgress, Release, ReleaseAsset};
pub use error::InstallError;
pub use plan::{InstallPlan, InstallProgress, InstallResult};
pub use platform::PlatformInfo;
pub use system::SystemIntegration;
pub use upgrade::{ExistingInstall, InstalledComponent};
