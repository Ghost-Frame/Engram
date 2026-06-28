//! System integration wizard step.
//!
//! Presents platform-appropriate service manager options (systemd on Linux,
//! launchd on macOS, or none). A collapsible preview shows the generated
//! service file content.

use eframe::egui;
use kleos_install_core::system::SystemIntegration;

use crate::theme;
use crate::wizard::InstallerApp;

/// Sentinel value for the system integration radio -- "none" option.
const RADIO_NONE: u8 = 0;
/// Sentinel value for the system integration radio -- systemd option.
const RADIO_SYSTEMD: u8 = 1;
/// Sentinel value for the system integration radio -- launchd option.
const RADIO_LAUNCHD: u8 = 2;

/// Draw the system integration configuration step.
///
/// Adapts the displayed options based on the detected platform: systemd is
/// shown on Linux, launchd on macOS, and "None" is always available. A
/// collapsible section previews the generated service file content.
pub fn draw_system_integration(ui: &mut egui::Ui, app: &mut InstallerApp) {
    ui.heading("System Integration");
    ui.add_space(4.0);
    ui.colored_label(
        theme::COLOR_TEXT_DIM,
        "Choose how the Kleos server should be registered with the system.",
    );
    ui.add_space(12.0);

    let has_systemd = app.platform_info.has_systemd;
    let has_launchd = app.platform_info.has_launchd;

    // Derive a simple u8 radio value from the enum for rendering.
    let mut radio_val = current_radio(&app.system_integration);

    ui.label("Service manager:");
    ui.add_space(4.0);

    // Always show "None".
    ui.radio_value(&mut radio_val, RADIO_NONE, "None (run manually)");
    ui.colored_label(
        theme::COLOR_TEXT_DIM,
        "    No service file is created. Start the server with `kleos-server` directly.",
    );
    ui.add_space(4.0);

    if has_systemd {
        ui.radio_value(&mut radio_val, RADIO_SYSTEMD, "systemd (user service)");
        ui.colored_label(
            theme::COLOR_TEXT_DIM,
            "    Installs a ~/.config/systemd/user/kleos-server.service unit file.",
        );
        ui.add_space(4.0);
    }

    if has_launchd {
        ui.radio_value(&mut radio_val, RADIO_LAUNCHD, "launchd (user agent)");
        ui.colored_label(
            theme::COLOR_TEXT_DIM,
            "    Installs a ~/Library/LaunchAgents/dev.syntheos.kleos-server.plist file.",
        );
        ui.add_space(4.0);
    }

    // Determine current auto_start from the enum.
    let mut auto_start = extract_auto_start(&app.system_integration);

    // Write changes back.
    app.system_integration = rebuild_integration(radio_val, auto_start);

    let has_service = radio_val != RADIO_NONE;
    ui.add_space(8.0);

    // Auto-start checkbox -- only enabled when a service is selected.
    ui.add_enabled_ui(has_service, |ui| {
        if ui
            .checkbox(&mut auto_start, "Start Kleos automatically on login")
            .changed()
        {
            app.system_integration = rebuild_integration(radio_val, auto_start);
        }
    });

    if !has_service {
        ui.colored_label(
            theme::COLOR_TEXT_DIM,
            "Auto-start requires a service manager to be selected.",
        );
    }

    ui.add_space(16.0);
    ui.separator();
    ui.add_space(8.0);

    // -- Service file preview --
    if has_service {
        let preview = generate_service_preview(app, radio_val);
        egui::CollapsingHeader::new("Preview service file")
            .default_open(false)
            .show(ui, |ui| {
                let mut preview_text = preview;
                ui.add(
                    egui::TextEdit::multiline(&mut preview_text)
                        .font(egui::TextStyle::Monospace)
                        .interactive(false)
                        .desired_width(f32::INFINITY),
                );
            });
    }
}

/// Map the current [`SystemIntegration`] variant to a radio sentinel value.
fn current_radio(integration: &SystemIntegration) -> u8 {
    match integration {
        SystemIntegration::Systemd { .. } => RADIO_SYSTEMD,
        SystemIntegration::Launchd { .. } => RADIO_LAUNCHD,
        _ => RADIO_NONE,
    }
}

/// Extract the auto_start flag from the current [`SystemIntegration`] variant.
///
/// Returns `false` for variants that do not carry an auto_start field.
fn extract_auto_start(integration: &SystemIntegration) -> bool {
    match integration {
        SystemIntegration::Systemd { auto_start } => *auto_start,
        SystemIntegration::Launchd { auto_start } => *auto_start,
        _ => false,
    }
}

/// Reconstruct a [`SystemIntegration`] from the radio sentinel and auto_start flag.
fn rebuild_integration(radio: u8, auto_start: bool) -> SystemIntegration {
    match radio {
        RADIO_SYSTEMD => SystemIntegration::Systemd { auto_start },
        RADIO_LAUNCHD => SystemIntegration::Launchd { auto_start },
        _ => SystemIntegration::None,
    }
}

/// Generate a preview of the service file that will be written.
///
/// Returns a systemd unit or launchd plist as a string, depending on the
/// radio selection.
fn generate_service_preview(app: &InstallerApp, radio_val: u8) -> String {
    let install_dir = app.install_dir.display();
    let config_dir = app.config_dir.display();

    match radio_val {
        RADIO_SYSTEMD => format!(
            "[Unit]\n\
             Description=Kleos Memory Server\n\
             After=network.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             WorkingDirectory={config_dir}\n\
             ExecStart={install_dir}/kleos-server --config {config_dir}/kleos.toml\n\
             EnvironmentFile={config_dir}/.env\n\
             Restart=on-failure\n\
             RestartSec=5s\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n"
        ),
        RADIO_LAUNCHD => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\"\n    \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
             \t<key>Label</key>\n\
             \t<string>dev.syntheos.kleos-server</string>\n\
             \t<key>ProgramArguments</key>\n\
             \t<array>\n\
             \t\t<string>{install_dir}/kleos-server</string>\n\
             \t\t<string>--config</string>\n\
             \t\t<string>{config_dir}/kleos.toml</string>\n\
             \t</array>\n\
             \t<key>KeepAlive</key>\n\
             \t<true/>\n\
             \t<key>RunAtLoad</key>\n\
             \t<true/>\n\
             </dict>\n\
             </plist>\n"
        ),
        _ => String::new(),
    }
}
