//! Platform detection and path resolution for the Kleos installer.

use std::path::PathBuf;

use crate::components::Platform;

/// Detected information about the current system environment.
#[derive(Debug, Clone)]
pub struct PlatformInfo {
    /// The resolved platform variant for this machine.
    pub platform: Platform,
    /// Human-readable OS name (e.g. "linux", "macos", "windows").
    pub os_name: String,
    /// CPU architecture string (e.g. "x86_64", "aarch64").
    pub arch: String,
    /// Whether systemd is available (Linux only, detected by `which systemctl`).
    pub has_systemd: bool,
    /// Whether launchd is available (true on macOS).
    pub has_launchd: bool,
    /// Default directory for installed Kleos binaries.
    pub default_install_dir: PathBuf,
    /// Default directory for Kleos configuration files (engram.toml, .env).
    pub default_config_dir: PathBuf,
    /// Default directory for Kleos runtime data files.
    pub default_data_dir: PathBuf,
}

impl PlatformInfo {
    /// Detect all platform information for the current machine.
    ///
    /// Reads OS and architecture from compile-time constants, probes for
    /// systemd / launchd availability, and computes default installation
    /// paths following XDG conventions on Unix and Windows conventions on
    /// Windows.
    pub fn detect() -> PlatformInfo {
        let os_name = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();
        let platform = Platform::detect();

        let has_systemd = which_exists("systemctl");
        let has_launchd = os_name == "macos";

        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

        let default_install_dir = if cfg!(windows) {
            home.join(".kleos").join("bin")
        } else {
            home.join(".local").join("bin")
        };

        let default_config_dir = if cfg!(windows) {
            home.join("AppData").join("Roaming").join("kleos")
        } else {
            xdg_config_dir().join("engram")
        };

        // Derive from XDG_DATA_HOME (or ~/.local/share on Unix, ~/AppData/Local on
        // Windows) so the path is absolute and stable regardless of the server's CWD.
        let default_data_dir = if cfg!(windows) {
            home.join("AppData")
                .join("Local")
                .join("kleos")
                .join("data")
        } else {
            xdg_data_dir().join("engram").join("data")
        };

        PlatformInfo {
            platform,
            os_name,
            arch,
            has_systemd,
            has_launchd,
            default_install_dir,
            default_config_dir,
            default_data_dir,
        }
    }
}

/// Check whether a command exists on PATH by attempting to find it via `which`.
///
/// Returns `true` if the command is found, `false` otherwise.
fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Resolve the XDG config directory, falling back to `~/.config` if the
/// environment variable is unset or empty.
fn xdg_config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config")
        })
}

/// Resolve the XDG data directory, falling back to `~/.local/share` if the
/// environment variable is unset or empty.
fn xdg_data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".local")
                .join("share")
        })
}
