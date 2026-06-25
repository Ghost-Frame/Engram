//! System integration (systemd, launchd, Windows Service) for the Kleos installer.

use std::path::{Path, PathBuf};

use crate::config::InstallerConfig;
use crate::error::InstallError;

/// The system-level service integration method chosen by the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemIntegration {
    /// Install a systemd user unit. Optionally enable and start it immediately.
    Systemd {
        /// Whether to run `systemctl --user enable --now` after installation.
        auto_start: bool,
    },
    /// Install a launchd plist in ~/Library/LaunchAgents. Optionally load it.
    Launchd {
        /// Whether to run `launchctl load` after installation.
        auto_start: bool,
    },
    /// Install as a Windows Service via `sc.exe`.
    WindowsService,
    /// Do not install any service integration -- the user will start the server manually.
    None,
}

/// Generate the content of a systemd user unit file for the Kleos server.
///
/// The unit runs `kleos-server` from `install_dir`, loading configuration from
/// `config_dir`. It is a simple `Type=simple` unit with `Restart=on-failure`.
pub fn generate_systemd_unit(
    _config: &InstallerConfig,
    install_dir: &Path,
    config_dir: &Path,
) -> String {
    let binary = install_dir.join("kleos-server");
    let env_file = config_dir.join(".env");
    let toml_file = config_dir.join("engram.toml");

    format!(
        r#"[Unit]
Description=Kleos Memory Server
After=network.target

[Service]
Type=simple
ExecStart={binary} --config {toml_file}
EnvironmentFile={env_file}
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=default.target
"#,
        binary = binary.display(),
        toml_file = toml_file.display(),
        env_file = env_file.display(),
    )
}

/// Generate the content of a launchd plist for the Kleos server.
///
/// Places a `KeepAlive=true` plist that runs `kleos-server` with the config
/// directory passed as `--config`. Logs go to `/tmp/kleos-server.log`.
pub fn generate_launchd_plist(
    _config: &InstallerConfig,
    install_dir: &Path,
    config_dir: &Path,
) -> String {
    let binary = install_dir.join("kleos-server");
    let toml_file = config_dir.join("engram.toml");

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
    "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.syntheos.kleos-server</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>--config</string>
        <string>{toml_file}</string>
    </array>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/kleos-server.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/kleos-server.log</string>
</dict>
</plist>
"#,
        // XML-escape the paths: an install/config path containing &, <, or >
        // would otherwise produce an invalid or injected plist.
        binary = xml_escape(&binary.display().to_string()),
        toml_file = xml_escape(&toml_file.display().to_string()),
    )
}

/// Escape a string for inclusion in XML element content (launchd plists).
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Write a systemd user unit to `~/.config/systemd/user/` and optionally enable it.
///
/// Creates the unit directory if it does not exist. If `auto_start` is `true`,
/// runs `systemctl --user enable --now kleos-server.service`. Returns
/// `InstallError::Io` on filesystem errors or `InstallError::Upgrade` if
/// systemctl fails.
pub fn install_systemd_unit(unit_content: &str, auto_start: bool) -> Result<(), InstallError> {
    let unit_dir = systemd_user_unit_dir()?;
    std::fs::create_dir_all(&unit_dir)?;

    let unit_path = unit_dir.join("kleos-server.service");
    std::fs::write(&unit_path, unit_content)?;

    if auto_start {
        let status = std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "kleos-server.service"])
            .status()
            .map_err(|e| InstallError::Upgrade(format!("failed to run systemctl: {e}")))?;

        if !status.success() {
            return Err(InstallError::Upgrade(
                "systemctl enable --now failed".to_string(),
            ));
        }
    }

    Ok(())
}

/// Write a launchd plist to `~/Library/LaunchAgents/` and optionally load it.
///
/// Creates the LaunchAgents directory if it does not exist. If `auto_start` is
/// `true`, runs `launchctl load` on the plist. Returns `InstallError::Io` on
/// filesystem errors or `InstallError::Upgrade` if launchctl fails.
pub fn install_launchd_plist(plist_content: &str, auto_start: bool) -> Result<(), InstallError> {
    let agents_dir = launch_agents_dir()?;
    std::fs::create_dir_all(&agents_dir)?;

    let plist_path = agents_dir.join("dev.syntheos.kleos-server.plist");
    std::fs::write(&plist_path, plist_content)?;

    if auto_start {
        let status = std::process::Command::new("launchctl")
            .arg("load")
            .arg(&plist_path)
            .status()
            .map_err(|e| InstallError::Upgrade(format!("failed to run launchctl: {e}")))?;

        if !status.success() {
            return Err(InstallError::Upgrade("launchctl load failed".to_string()));
        }
    }

    Ok(())
}

/// Resolve the systemd user unit directory (`~/.config/systemd/user/`).
fn systemd_user_unit_dir() -> Result<PathBuf, InstallError> {
    let home = dirs::home_dir()
        .ok_or_else(|| InstallError::Platform("cannot determine home directory".to_string()))?;
    Ok(home.join(".config").join("systemd").join("user"))
}

/// Resolve the launchd LaunchAgents directory (`~/Library/LaunchAgents/`).
fn launch_agents_dir() -> Result<PathBuf, InstallError> {
    let home = dirs::home_dir()
        .ok_or_else(|| InstallError::Platform("cannot determine home directory".to_string()))?;
    Ok(home.join("Library").join("LaunchAgents"))
}
