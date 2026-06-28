//! Main application state and eframe integration for the Kleos installer wizard.
//!
//! The wizard is a linear 6-step flow. Each step is rendered by a dedicated
//! function in the `steps` sub-modules. Navigation is driven by "Back" and
//! "Next" buttons rendered in the bottom panel.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use eframe::egui;
use kleos_install_core::system::SystemIntegration;
use kleos_install_core::{
    profile_components, EmbeddingConfig, ExistingInstall, InstallPlan, InstallerConfig,
    PlatformInfo, Profile, RerankerConfig, SecurityConfig, ServerConfig,
};

use crate::steps;
use crate::theme;

// ---------------------------------------------------------------------------
// Step enum
// ---------------------------------------------------------------------------

/// Ordered enumeration of all wizard steps.
///
/// The steps are presented in sequence; the user navigates forward and back
/// using the bottom-panel buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// Component and profile selection.
    Components,
    /// Server bind address, port, and data directory.
    ServerConfig,
    /// Embedding and reranker provider selection.
    Embeddings,
    /// Security key generation and access control.
    Security,
    /// System service integration (systemd / launchd / none).
    SystemIntegration,
    /// Advanced (expert) server toggles.
    Advanced,
    /// Plan summary and installation trigger.
    Summary,
}

/// Step-ordering and display helpers for the GUI wizard flow.
impl WizardStep {
    /// Human-readable label shown in the step indicator bar.
    pub fn label(self) -> &'static str {
        match self {
            WizardStep::Components => "Components",
            WizardStep::ServerConfig => "Server",
            WizardStep::Embeddings => "Embeddings",
            WizardStep::Security => "Security",
            WizardStep::SystemIntegration => "System",
            WizardStep::Advanced => "Advanced",
            WizardStep::Summary => "Install",
        }
    }

    /// Ordered list of all wizard steps.
    pub fn all() -> Vec<WizardStep> {
        vec![
            WizardStep::Components,
            WizardStep::ServerConfig,
            WizardStep::Embeddings,
            WizardStep::Security,
            WizardStep::SystemIntegration,
            WizardStep::Advanced,
            WizardStep::Summary,
        ]
    }
}

// ---------------------------------------------------------------------------
// Install result
// ---------------------------------------------------------------------------

/// Outcome of a completed installation run.
#[derive(Debug, Clone)]
pub struct InstallResult {
    /// Human-readable summary of what was installed.
    pub summary: String,
    /// The initial API key generated for this installation.
    pub api_key: String,
    /// The base URL the server will listen on.
    pub server_url: String,
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// Main application state shared across all wizard steps.
///
/// Implements [`eframe::App`] and drives the immediate-mode render loop.
pub struct InstallerApp {
    // -- Wizard navigation --
    /// The step currently being displayed.
    pub current_step: WizardStep,
    /// Ordered list of all steps (determines progress-bar rendering).
    pub steps: Vec<WizardStep>,

    // -- User selections --
    /// Component IDs the user has selected for installation.
    pub selected_components: Vec<String>,
    /// The profile that seeded the component selection, if any.
    pub selected_profile: Option<Profile>,
    /// Security keys and access-control settings.
    pub security_config: SecurityConfig,
    /// System service integration choice.
    pub system_integration: SystemIntegration,
    /// Directory where binaries will be placed.
    pub install_dir: PathBuf,
    /// Directory where configuration files will be written.
    pub config_dir: PathBuf,
    /// Target Kleos version string.
    pub version: String,

    // -- Detected environment --
    /// Platform detection results.
    pub platform_info: PlatformInfo,
    /// An existing Kleos installation detected on this machine, if any.
    pub existing_install: Option<ExistingInstall>,

    // -- GUI-specific embedding/reranker state --
    /// When `true` the embedding provider is local ONNX; when `false` it is remote.
    pub embedding_provider_local: bool,
    /// Reranker mode: 0 = local ONNX, 1 = remote, 2 = disabled.
    pub reranker_mode: u8,

    // -- Remote embedding text buffers --
    /// URL for a remote embedding endpoint.
    pub remote_embed_url: String,
    /// API key for a remote embedding endpoint.
    pub remote_embed_api_key: String,
    /// Model name for a remote embedding endpoint.
    pub remote_embed_model: String,
    /// Output dimension for a remote embedding endpoint.
    pub remote_embed_dimension: String,

    // -- Remote reranker text buffers --
    /// URL for a remote reranker endpoint.
    pub remote_reranker_url: String,
    /// API key for a remote reranker endpoint.
    pub remote_reranker_api_key: String,
    /// Model name for a remote reranker endpoint.
    pub remote_reranker_model: String,

    // -- Server config text buffers (String so egui can edit them) --
    /// Text buffer for the server host field.
    pub server_host_buf: String,
    /// Text buffer for the server port field.
    pub server_port_buf: String,
    /// Text buffer for the server data directory field.
    pub server_data_dir_buf: String,
    /// Text buffer for the database filename field.
    pub server_db_path_buf: String,
    /// Text buffer for the CORS origins field.
    pub server_cors_buf: String,

    // -- Install execution state --
    /// Whether the background install thread is currently running.
    pub install_running: bool,
    /// Progress log lines collected from the background thread.
    pub install_progress: Vec<String>,
    /// Shared progress channel written by the background thread.
    pub install_progress_channel: Arc<Mutex<Vec<String>>>,
    /// Final result of the installation (set when the thread finishes).
    pub install_result: Option<Result<InstallResult, String>>,

    // -- UI transient state --
    /// Whether the "are you sure you want to quit?" confirmation is visible.
    pub show_quit_confirm: bool,

    // -- Advanced toggles --
    /// Expert toggles collected on the Advanced step, folded into the plan.
    pub advanced: steps::advanced::AdvancedToggles,
}

/// Construction, per-step rendering, and plan assembly for the GUI app.
impl InstallerApp {
    /// Construct a new [`InstallerApp`] with sensible defaults.
    ///
    /// Detects the current platform, generates initial security keys, and
    /// applies the Kleos visual theme to the egui context.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply_theme(&cc.egui_ctx);

        let platform_info = PlatformInfo::detect();
        let install_dir = platform_info.default_install_dir.clone();
        let config_dir = platform_info.default_config_dir.clone();

        let security_config = generate_security_config();

        let existing_install = kleos_install_core::upgrade::detect_existing_install();

        let selected_components = profile_components(Profile::Server)
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        let server_config = ServerConfig::default();
        let server_host_buf = server_config.host.clone();
        let server_port_buf = server_config.port.to_string();
        let server_data_dir_buf = server_config.data_dir.display().to_string();
        let server_db_path_buf = server_config.db_path.clone();
        let server_cors_buf = server_config.cors_origins.clone().unwrap_or_default();

        InstallerApp {
            current_step: WizardStep::Components,
            steps: WizardStep::all(),
            selected_components,
            selected_profile: Some(Profile::Server),
            security_config,
            system_integration: SystemIntegration::None,
            install_dir,
            config_dir,
            version: "latest".to_string(),
            platform_info,
            existing_install,
            embedding_provider_local: true,
            reranker_mode: 2,
            remote_embed_url: String::new(),
            remote_embed_api_key: String::new(),
            remote_embed_model: String::new(),
            remote_embed_dimension: "1024".to_string(),
            remote_reranker_url: String::new(),
            remote_reranker_api_key: String::new(),
            remote_reranker_model: String::new(),
            server_host_buf,
            server_port_buf,
            server_data_dir_buf,
            server_db_path_buf,
            server_cors_buf,
            install_running: false,
            install_progress: Vec::new(),
            install_progress_channel: Arc::new(Mutex::new(Vec::new())),
            install_result: None,
            show_quit_confirm: false,
            advanced: steps::advanced::AdvancedToggles::default(),
        }
    }

    /// Move to the next wizard step, if one exists.
    pub fn go_next(&mut self) {
        let idx = self.steps.iter().position(|&s| s == self.current_step);
        if let Some(i) = idx {
            if i + 1 < self.steps.len() {
                self.current_step = self.steps[i + 1];
            }
        }
    }

    /// Move to the previous wizard step, if one exists.
    pub fn go_back(&mut self) {
        let idx = self.steps.iter().position(|&s| s == self.current_step);
        if let Some(i) = idx {
            if i > 0 {
                self.current_step = self.steps[i - 1];
            }
        }
    }

    /// Return `true` when the current step is the first one.
    pub fn is_first_step(&self) -> bool {
        self.current_step == *self.steps.first().unwrap_or(&WizardStep::Components)
    }

    /// Return `true` when the current step is the last one (Summary).
    pub fn is_last_step(&self) -> bool {
        self.current_step == *self.steps.last().unwrap_or(&WizardStep::Summary)
    }

    /// Build the embedding config from current GUI state.
    fn build_embedding_config(&self) -> EmbeddingConfig {
        if self.embedding_provider_local {
            EmbeddingConfig::LocalOnnx {
                model: "BAAI/bge-m3".to_string(),
                dimension: 1024,
                auto_download: true,
            }
        } else {
            EmbeddingConfig::Remote {
                url: self.remote_embed_url.clone(),
                api_key: self.remote_embed_api_key.clone(),
                model: if self.remote_embed_model.is_empty() {
                    None
                } else {
                    Some(self.remote_embed_model.clone())
                },
                dimension: self.remote_embed_dimension.parse().unwrap_or(1024),
            }
        }
    }

    /// Build the reranker config from current GUI state.
    fn build_reranker_config(&self) -> RerankerConfig {
        match self.reranker_mode {
            0 => RerankerConfig::LocalOnnx,
            1 => RerankerConfig::Remote {
                endpoint: self.remote_reranker_url.clone(),
                api_key: self.remote_reranker_api_key.clone(),
                model: self.remote_reranker_model.clone(),
            },
            _ => RerankerConfig::Disabled,
        }
    }

    /// Build the server config from current text buffers.
    fn build_server_config(&self) -> ServerConfig {
        ServerConfig {
            host: if self.server_host_buf.is_empty() {
                "127.0.0.1".to_string()
            } else {
                self.server_host_buf.clone()
            },
            port: self.server_port_buf.parse().unwrap_or(4200),
            data_dir: PathBuf::from(&self.server_data_dir_buf),
            db_path: self.server_db_path_buf.clone(),
            cors_origins: if self.server_cors_buf.is_empty() {
                None
            } else {
                Some(self.server_cors_buf.clone())
            },
        }
    }

    /// Build an [`InstallPlan`] from the current wizard state.
    ///
    /// Server, embedding, and reranker configs are only included when
    /// `kleos-server` is among the selected components.
    pub fn build_plan(&self) -> InstallPlan {
        let has_server = self.selected_components.iter().any(|c| c == "kleos-server");

        let installer_config = InstallerConfig {
            server: if has_server {
                Some(self.build_server_config())
            } else {
                None
            },
            embedding: if has_server {
                Some(self.build_embedding_config())
            } else {
                None
            },
            reranker: if has_server {
                Some(self.build_reranker_config())
            } else {
                None
            },
            security: self.security_config.clone(),
            overrides: self.advanced.to_overrides(),
        };

        InstallPlan {
            components: self.selected_components.clone(),
            install_dir: self.install_dir.clone(),
            config_dir: self.config_dir.clone(),
            version: self.version.clone(),
            config: installer_config,
            system_integration: self.system_integration.clone(),
            is_upgrade: self.existing_install.is_some(),
        }
    }

    /// Poll the shared progress channel and drain any new messages into
    /// [`InstallerApp::install_progress`].
    pub fn poll_progress(&mut self) {
        if let Ok(mut ch) = self.install_progress_channel.lock() {
            let new: Vec<String> = ch.drain(..).collect();
            self.install_progress.extend(new);
        }
    }
}

/// eframe entry point: drives one render frame of the installer per update.
impl eframe::App for InstallerApp {
    /// Render the installer for one frame.
    ///
    /// Draws the step-indicator panel at the top, the current step's content
    /// in the central area, and the navigation controls in a bottom panel.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain progress messages from background thread every frame.
        if self.install_running {
            self.poll_progress();
            ctx.request_repaint();
        }

        // -- Top panel: step indicator --
        egui::TopBottomPanel::top("step_indicator").show(ctx, |ui| {
            draw_step_indicator(ui, self);
        });

        // -- Bottom panel: navigation buttons --
        egui::TopBottomPanel::bottom("nav_buttons").show(ctx, |ui| {
            draw_nav_buttons(ui, self);
        });

        // -- Central panel: current step content --
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| match self.current_step {
                WizardStep::Components => steps::components::draw_components(ui, self),
                WizardStep::ServerConfig => steps::server::draw_server_config(ui, self),
                WizardStep::Embeddings => steps::embeddings::draw_embeddings(ui, self),
                WizardStep::Security => steps::security::draw_security(ui, self),
                WizardStep::SystemIntegration => steps::system::draw_system_integration(ui, self),
                WizardStep::Advanced => steps::advanced::draw_advanced(ui, self),
                WizardStep::Summary => steps::summary::draw_summary(ui, self),
            });
        });

        // -- Quit confirmation modal --
        if self.show_quit_confirm {
            egui::Window::new("Quit Installer?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label("Installation is not complete. Are you sure you want to quit?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.button("Cancel").clicked() {
                            self.show_quit_confirm = false;
                        }
                    });
                });
        }
    }
}

// ---------------------------------------------------------------------------
// Internal render helpers
// ---------------------------------------------------------------------------

/// Draw the horizontal step-indicator bar at the top of the window.
///
/// Each step is shown as a labelled node. The active step is highlighted with
/// the accent colour; completed steps use a dimmed style.
fn draw_step_indicator(ui: &mut egui::Ui, app: &InstallerApp) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let active_idx = app
            .steps
            .iter()
            .position(|&s| s == app.current_step)
            .unwrap_or(0);

        for (i, &step) in app.steps.iter().enumerate() {
            let is_active = i == active_idx;
            let is_done = i < active_idx;

            let color = if is_active {
                theme::COLOR_ACCENT
            } else if is_done {
                theme::COLOR_ACCENT_DIM
            } else {
                theme::COLOR_TEXT_DIM
            };

            ui.colored_label(color, format!("{}", i + 1));
            ui.colored_label(color, step.label());

            if i + 1 < app.steps.len() {
                ui.colored_label(theme::COLOR_TEXT_DIM, " > ");
            }
        }
    });
    ui.add_space(4.0);
    ui.separator();
}

/// Draw the navigation bar (Back / Next / Cancel) at the bottom of the window.
///
/// The "Back" button is hidden on the first step. The "Next" button becomes
/// "Install" when on the final Summary step. Cancel shows the quit
/// confirmation unless the install is already complete.
fn draw_nav_buttons(ui: &mut egui::Ui, app: &mut InstallerApp) {
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // Cancel / Close button on the left.
        let close_label = if app.install_result.is_some() {
            "Close"
        } else {
            "Cancel"
        };
        if ui.button(close_label).clicked() {
            if app.install_result.is_some() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                app.show_quit_confirm = true;
            }
        }

        // Push Back / Next to the right.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Only show Next/Install when not in the middle of installing.
            if !app.install_running && app.install_result.is_none() {
                let next_label = if app.is_last_step() {
                    "Install"
                } else {
                    "Next >"
                };

                if ui.button(next_label).clicked() {
                    if app.is_last_step() {
                        app.install_running = true;
                        start_install(app);
                    } else {
                        app.go_next();
                    }
                }
            }

            if !app.is_first_step()
                && !app.install_running
                && app.install_result.is_none()
                && ui.button("< Back").clicked()
            {
                app.go_back();
            }
        });
    });
    ui.add_space(4.0);
}

/// Spawn a background thread to execute the install plan.
///
/// Progress messages are pushed into the shared channel so the UI thread can
/// drain them each frame. The completion sentinel "INSTALL_DONE" or
/// "INSTALL_ERROR:..." is used by the summary step to detect completion.
fn start_install(app: &mut InstallerApp) {
    let plan = app.build_plan();
    let channel = Arc::clone(&app.install_progress_channel);

    let _ = std::thread::spawn(move || {
        let ch = Arc::clone(&channel);
        let progress = ChannelProgress { channel: ch };

        match plan.execute(&progress, "") {
            Ok(result) => {
                let push = |msg: &str| {
                    if let Ok(mut ch) = channel.lock() {
                        ch.push(msg.to_string());
                    }
                };
                push(&format!(
                    "INSTALL_DONE:{}:{}",
                    result.api_key,
                    result.server_url.unwrap_or_default()
                ));
            }
            Err(e) => {
                if let Ok(mut ch) = channel.lock() {
                    ch.push(format!("INSTALL_ERROR:{e}"));
                }
            }
        }
    });
}

/// Adapter that forwards [`InstallProgress`] callbacks to the shared channel.
struct ChannelProgress {
    /// Shared channel written by the background thread, read by the UI.
    channel: Arc<Mutex<Vec<String>>>,
}

/// Forwards installer progress events into the shared log channel the GUI polls.
impl kleos_install_core::InstallProgress for ChannelProgress {
    /// Record the start of an installation phase.
    fn on_phase(&self, phase: &str, detail: &str) {
        if let Ok(mut ch) = self.channel.lock() {
            ch.push(format!("[{phase}] {detail}"));
        }
    }

    /// Record a download progress percentage for a component.
    fn on_download_progress(&self, component: &str, bytes: u64, total: u64) {
        if let Some(pct) = (bytes * 100).checked_div(total) {
            if let Ok(mut ch) = self.channel.lock() {
                ch.push(format!("  {component}: {pct}%"));
            }
        }
    }

    /// Record that a component finished installing.
    fn on_component_installed(&self, component: &str) {
        if let Ok(mut ch) = self.channel.lock() {
            ch.push(format!("  Installed {component}"));
        }
    }

    /// Record successful completion of the whole install.
    fn on_complete(&self) {
        if let Ok(mut ch) = self.channel.lock() {
            ch.push("Installation complete.".to_string());
        }
    }

    /// Record a fatal installation error.
    fn on_error(&self, error: &kleos_install_core::InstallError) {
        if let Ok(mut ch) = self.channel.lock() {
            ch.push(format!("ERROR: {error}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Key generation
// ---------------------------------------------------------------------------

/// Generate a new [`SecurityConfig`] with cryptographically random keys.
///
/// Delegates to `kleos_install_core::security::generate_hex_key` which uses
/// the OS CSPRNG via the `rand` crate.
fn generate_security_config() -> SecurityConfig {
    use kleos_install_core::security;
    SecurityConfig {
        encryption_key: security::generate_encryption_key(),
        api_key_pepper: security::generate_api_key_pepper(),
        initial_api_key: security::generate_api_key(),
        hmac_secret: security::generate_hmac_secret(),
        open_access: false,
    }
}
