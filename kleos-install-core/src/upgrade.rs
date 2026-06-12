//! Upgrade detection and config migration for the Kleos installer.

use std::path::{Path, PathBuf};

use crate::config::{InstallerConfig, RerankerConfig, SecurityConfig, ServerConfig};
use crate::error::InstallError;

/// Information about an existing Kleos installation found on the system.
#[derive(Debug, Clone)]
pub struct ExistingInstall {
    /// Directory where the Kleos binaries are installed.
    pub install_dir: PathBuf,
    /// Path to the existing `engram.toml` config file, if found.
    pub config_path: Option<PathBuf>,
    /// List of component binaries detected in `install_dir`.
    pub components: Vec<InstalledComponent>,
}

/// A single component binary found in an existing installation.
#[derive(Debug, Clone)]
pub struct InstalledComponent {
    /// The component's machine-readable ID (matches the component registry).
    pub id: String,
    /// The version string reported by `<binary> --version`, if available.
    pub version: Option<String>,
    /// Full path to the binary on disk.
    pub path: PathBuf,
}

/// Common locations to probe for an existing Kleos installation.
///
/// Checked in order; the first directory containing at least one known binary
/// is treated as the installation root.
const PROBE_DIRS: &[&str] = &["~/.local/bin", "/usr/local/bin", "/opt/kleos/bin"];

/// Known binary names to look for when probing an installation directory.
const KNOWN_BINARIES: &[&str] = &[
    "kleos-server",
    "kleos-cli",
    "kleos-mcp",
    "kleos-credd",
    "cred",
    "kleos-sidecar",
    "agent-forge",
    "eidolon-supervisor",
    "kleos-sh",
    "kleos-ingest",
];

/// Scan common install directories for an existing Kleos installation.
///
/// Returns `Some(ExistingInstall)` if at least one known Kleos binary is found,
/// or `None` if no installation is detected.
pub fn detect_existing_install() -> Option<ExistingInstall> {
    let home = dirs::home_dir()?;

    for &dir_str in PROBE_DIRS {
        let dir = if dir_str.starts_with('~') {
            home.join(&dir_str[2..])
        } else {
            PathBuf::from(dir_str)
        };

        if !dir.exists() {
            continue;
        }

        let mut components = Vec::new();
        for &name in KNOWN_BINARIES {
            let path = dir.join(name);
            let win_path = dir.join(format!("{name}.exe"));
            let found = if path.exists() {
                Some(path)
            } else if win_path.exists() {
                Some(win_path)
            } else {
                None
            };

            if let Some(bin_path) = found {
                let version = read_binary_version(&bin_path);
                components.push(InstalledComponent {
                    id: name.to_string(),
                    version,
                    path: bin_path,
                });
            }
        }

        if !components.is_empty() {
            let config_path = find_config_file(&home);
            return Some(ExistingInstall {
                install_dir: dir,
                config_path,
                components,
            });
        }
    }

    None
}

/// Parse an existing `engram.toml` into an `InstallerConfig`.
///
/// Reads the TOML file and maps known fields back to the installer's in-memory
/// representation. Returns `InstallError::Upgrade` if the file cannot be parsed.
///
/// Note: security secrets are not stored in engram.toml, so `SecurityConfig`
/// fields will be empty strings; callers should regenerate secrets for upgrades.
pub fn read_existing_config(config_path: &Path) -> Result<InstallerConfig, InstallError> {
    let content = std::fs::read_to_string(config_path)?;
    let table: toml::Value = content
        .parse()
        .map_err(|e| InstallError::Upgrade(format!("failed to parse engram.toml: {e}")))?;

    let server = table.get("server").map(|s| ServerConfig {
        host: string_field(s, "host").unwrap_or_else(|| "127.0.0.1".to_string()),
        port: s
            .get("port")
            .and_then(|v| v.as_integer())
            .map(|v| v as u16)
            .unwrap_or(4200),
        data_dir: string_field(s, "data_dir")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data")),
        db_path: string_field(s, "db_path").unwrap_or_else(|| "kleos.db".to_string()),
        cors_origins: string_field(s, "cors_origins"),
    });

    let reranker = table
        .get("reranker")
        .map(|r| match string_field(r, "provider").as_deref() {
            Some("disabled") | None => RerankerConfig::Disabled,
            Some("local_onnx") => RerankerConfig::LocalOnnx,
            _ => RerankerConfig::Disabled,
        });

    // Security secrets are not written to TOML -- return empty placeholders.
    let security = SecurityConfig {
        encryption_key: String::new(),
        api_key_pepper: String::new(),
        initial_api_key: String::new(),
        hmac_secret: String::new(),
        open_access: table
            .get("security")
            .and_then(|s| s.get("open_access"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };

    Ok(InstallerConfig {
        server,
        embedding: None,
        reranker,
        security,
    })
}

/// Create a timestamped backup of a config file by copying it to `<path>.bak.<timestamp>`.
///
/// Returns the path of the newly created backup file.
pub fn backup_config(config_path: &Path) -> Result<PathBuf, InstallError> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let backup_path = config_path.with_extension(format!("bak.{timestamp}"));
    std::fs::copy(config_path, &backup_path)?;
    Ok(backup_path)
}

/// Path of the version marker written alongside an installed binary
/// (`<binary>.version`). Reading the recorded version from this file avoids
/// executing the binary to learn its version: the probe dirs include
/// user-writable locations (e.g. `~/.local/bin`), so executing a discovered
/// binary would run attacker-planted code -- a privilege-escalation vector when
/// the installer runs as root.
pub(crate) fn version_marker_path(bin: &Path) -> PathBuf {
    let mut s = bin.as_os_str().to_owned();
    s.push(".version");
    PathBuf::from(s)
}

/// Read the recorded version of an installed binary from its `<binary>.version`
/// marker. Returns `None` if no marker exists. The binary is never executed.
fn read_binary_version(path: &Path) -> Option<String> {
    std::fs::read_to_string(version_marker_path(path))
        .ok()
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
        .filter(|s| !s.is_empty())
}

/// Search common config locations for an `engram.toml` file.
///
/// Returns the first path found, or `None` if none exist.
fn find_config_file(home: &Path) -> Option<PathBuf> {
    let candidates = [
        home.join(".config").join("engram").join("engram.toml"),
        home.join(".kleos").join("engram.toml"),
        PathBuf::from("/etc/kleos/engram.toml"),
    ];

    candidates.into_iter().find(|p| p.exists())
}

/// Extract a string field from a TOML value map.
///
/// Returns `None` if the key is absent or the value is not a string.
fn string_field(table: &toml::Value, key: &str) -> Option<String> {
    table.get(key)?.as_str().map(|s| s.to_string())
}
