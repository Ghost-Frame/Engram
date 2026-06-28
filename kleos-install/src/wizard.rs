//! Wizard state machine and main event loop for the Kleos TUI installer.
//!
//! The wizard is a linear sequence of steps. Each step has its own local UI
//! state managed by its step module. `WizardState` holds the cross-step data
//! (component selections, config values, etc.) that is shared between steps and
//! assembled into an `InstallPlan` at the end.

use std::path::PathBuf;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use kleos_install_core::config::{
    EmbeddingConfig, InstallerConfig, RerankerConfig, SecurityConfig, ServerConfig,
};
use kleos_install_core::plan::{InstallPlan, InstallResult};
use kleos_install_core::security;
use kleos_install_core::system::SystemIntegration;
use kleos_install_core::upgrade::ExistingInstall;
use kleos_install_core::{profile_components, PlatformInfo, Profile};
use ratatui::{
    backend::Backend,
    layout::{Constraint, Direction, Layout},
    Terminal,
};

use crate::steps::{
    advanced::{draw_advanced_step, handle_advanced_input, AdvancedStepState},
    components::{draw_components_step, handle_components_input, ComponentsStepState},
    embeddings::{draw_embeddings_step, handle_embeddings_input, EmbeddingsStepState},
    security::{draw_security_step, handle_security_input, SecurityStepState},
    server::{draw_server_step, handle_server_input, ServerStepState},
    summary::{draw_summary_step, handle_summary_input, SummaryStepState},
    system::{draw_system_step, handle_system_input, SystemStepState},
};
use crate::tui::{draw_navigation_bar, draw_quit_confirm, draw_step_indicator, draw_title};
use crate::types::StepResult;

/// All steps available in the installer wizard.
///
/// Steps may be conditionally hidden depending on which components the user
/// selects in the Components step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    /// Component and profile selection.
    Components,
    /// Server host, port, and path configuration.
    ServerConfig,
    /// Embedding and reranker provider configuration.
    Embeddings,
    /// API key and security settings.
    Security,
    /// System service manager integration.
    SystemIntegration,
    /// Advanced (expert) server toggles.
    Advanced,
    /// Final summary and install confirmation.
    Summary,
}

/// Step-ordering and display helpers for the TUI wizard flow.
impl WizardStep {
    /// Return the short display label used in the step indicator bar.
    pub fn label(self) -> &'static str {
        match self {
            WizardStep::Components => "Components",
            WizardStep::ServerConfig => "Server",
            WizardStep::Embeddings => "Embeddings",
            WizardStep::Security => "Security",
            WizardStep::SystemIntegration => "System",
            WizardStep::Advanced => "Advanced",
            WizardStep::Summary => "Summary",
        }
    }
}

/// All local per-step UI states bundled together for lifetime convenience.
///
/// Only the active step's state is used at any given time. Keeping them all
/// here avoids passing separate references through the event loop.
struct StepStates {
    /// State for the component selection step.
    components: ComponentsStepState,
    /// State for the server configuration step.
    server: ServerStepState,
    /// State for the embeddings configuration step.
    embeddings: EmbeddingsStepState,
    /// State for the security configuration step.
    security: SecurityStepState,
    /// State for the system integration step.
    system: SystemStepState,
    /// State for the advanced toggle step.
    advanced: AdvancedStepState,
    /// State for the summary and install step.
    summary: SummaryStepState,
}

/// Cross-step shared state for the entire wizard session.
///
/// This accumulates user choices from each step and is eventually assembled
/// into an `InstallPlan` when the user confirms on the Summary step.
pub struct WizardState {
    /// The currently active step being rendered and handled.
    pub current_step: WizardStep,
    /// The ordered list of steps visible to the user (may be a subset of all steps).
    pub steps: Vec<WizardStep>,
    /// Component IDs the user has selected for installation.
    pub selected_components: Vec<String>,
    /// The installation profile last chosen by the user.
    pub selected_profile: Option<Profile>,
    /// Server configuration collected from the ServerConfig step.
    pub server_config: ServerConfig,
    /// Embedding provider configuration from the Embeddings step.
    pub embedding_config: Option<EmbeddingConfig>,
    /// Reranker configuration from the Embeddings step.
    pub reranker_config: Option<RerankerConfig>,
    /// Security keys and access settings from the Security step.
    pub security_config: SecurityConfig,
    /// System service integration settings from the System step.
    pub system_integration: SystemIntegration,
    /// Directory where binaries will be installed.
    pub install_dir: PathBuf,
    /// Directory where configuration files will be written.
    pub config_dir: PathBuf,
    /// Target version string ("latest" or a semver tag).
    pub version: String,
    /// Detected information about the current platform.
    pub platform_info: PlatformInfo,
    /// Existing installation detected on the system, if any.
    #[allow(dead_code)]
    pub existing_install: Option<ExistingInstall>,
    /// Whether the quit confirmation dialog is currently shown.
    pub show_quit_confirm: bool,
    /// Whether this is an upgrade of an existing installation.
    pub is_upgrade: bool,
    /// Overrides collected from the Advanced step, folded into the plan.
    pub advanced_overrides: kleos_install_core::config::ConfigOverrides,
}

/// Construction and plan assembly for the TUI wizard's mutable state.
impl WizardState {
    /// Build the initial wizard state from platform info and CLI overrides.
    ///
    /// Detects existing installations, populates defaults, and constructs the
    /// initial step list. Security keys are auto-generated using the OS CSPRNG.
    pub fn new(
        platform_info: PlatformInfo,
        version: Option<String>,
        install_dir: Option<PathBuf>,
    ) -> Self {
        let install_dir = install_dir.unwrap_or_else(|| platform_info.default_install_dir.clone());
        let config_dir = platform_info.default_config_dir.clone();
        let version = version.unwrap_or_else(|| "latest".to_string());

        let existing_install = kleos_install_core::upgrade::detect_existing_install();
        let is_upgrade = existing_install.is_some();

        let server_config = ServerConfig {
            data_dir: platform_info.default_data_dir.clone(),
            db_path: "kleos.db".to_string(),
            ..ServerConfig::default()
        };

        let security_config = SecurityConfig {
            encryption_key: security::generate_encryption_key(),
            api_key_pepper: security::generate_api_key_pepper(),
            initial_api_key: security::generate_api_key(),
            hmac_secret: security::generate_hmac_secret(),
            open_access: false,
        };

        // Seed component selection from Server profile.
        let profile_ids = profile_components(Profile::Server);
        let selected_components: Vec<String> = profile_ids.iter().map(|s| s.to_string()).collect();

        let system_integration = auto_detect_system_integration(&platform_info);

        let mut state = WizardState {
            current_step: WizardStep::Components,
            steps: Vec::new(),
            selected_components,
            selected_profile: Some(Profile::Server),
            server_config,
            embedding_config: Some(EmbeddingConfig::LocalOnnx {
                model: "BAAI/bge-m3".to_string(),
                dimension: 1024,
                auto_download: true,
            }),
            reranker_config: Some(RerankerConfig::Disabled),
            security_config,
            system_integration,
            install_dir,
            config_dir,
            version,
            platform_info,
            existing_install,
            show_quit_confirm: false,
            is_upgrade,
            advanced_overrides: kleos_install_core::config::ConfigOverrides::default(),
        };
        state.rebuild_steps();
        state
    }

    /// Rebuild the ordered step list based on current component selection.
    ///
    /// ServerConfig and Embeddings steps are only shown when `kleos-server` is
    /// selected. SystemIntegration is only shown on Unix platforms when the
    /// server is selected.
    pub fn rebuild_steps(&mut self) {
        let has_server = self.selected_components.iter().any(|c| c == "kleos-server");
        let has_systemd_or_launchd =
            self.platform_info.has_systemd || self.platform_info.has_launchd;

        let mut steps = vec![WizardStep::Components];

        if has_server {
            steps.push(WizardStep::ServerConfig);
            steps.push(WizardStep::Embeddings);
        }

        steps.push(WizardStep::Security);

        if has_systemd_or_launchd && has_server {
            steps.push(WizardStep::SystemIntegration);
        }

        // Advanced server toggles are only relevant when a server is installed.
        if has_server {
            steps.push(WizardStep::Advanced);
        }

        steps.push(WizardStep::Summary);

        // If current step was removed, reset to first step.
        if !steps.contains(&self.current_step) {
            self.current_step = steps[0];
        }

        self.steps = steps;
    }

    /// Advance to the next step.
    ///
    /// Returns `true` if advanced successfully, `false` if already on the last step.
    pub fn next_step(&mut self) -> bool {
        let idx = self.steps.iter().position(|s| *s == self.current_step);
        if let Some(i) = idx {
            if i + 1 < self.steps.len() {
                self.current_step = self.steps[i + 1];
                return true;
            }
        }
        false
    }

    /// Return to the previous step.
    ///
    /// Returns `true` if went back, `false` if already on the first step.
    pub fn prev_step(&mut self) -> bool {
        let idx = self.steps.iter().position(|s| *s == self.current_step);
        if let Some(i) = idx {
            if i > 0 {
                self.current_step = self.steps[i - 1];
                return true;
            }
        }
        false
    }

    /// Assemble the final `InstallPlan` from all collected wizard state.
    pub fn build_plan(&self) -> InstallPlan {
        let config = InstallerConfig {
            server: if self.selected_components.iter().any(|c| c == "kleos-server") {
                Some(self.server_config.clone())
            } else {
                None
            },
            embedding: self.embedding_config.clone(),
            reranker: self.reranker_config.clone(),
            security: self.security_config.clone(),
            overrides: self.advanced_overrides.clone(),
        };

        InstallPlan {
            components: self.selected_components.clone(),
            install_dir: self.install_dir.clone(),
            config_dir: self.config_dir.clone(),
            version: self.version.clone(),
            config,
            system_integration: self.system_integration.clone(),
            is_upgrade: self.is_upgrade,
        }
    }

    /// Return `true` if the current step is the first step.
    pub fn is_first_step(&self) -> bool {
        self.steps.first() == Some(&self.current_step)
    }
}

/// Choose a default system integration based on detected platform capabilities.
fn auto_detect_system_integration(platform: &PlatformInfo) -> SystemIntegration {
    if platform.has_systemd {
        SystemIntegration::Systemd { auto_start: true }
    } else if platform.has_launchd {
        SystemIntegration::Launchd { auto_start: true }
    } else {
        SystemIntegration::None
    }
}

/// Run the interactive wizard event loop.
///
/// Draws the wizard, handles keyboard events, and returns either a completed
/// `InstallResult` (when the user confirms and install succeeds) or `None` when
/// the user quits.
pub async fn run_wizard<B: Backend>(
    terminal: &mut Terminal<B>,
    version: Option<String>,
    install_dir: Option<PathBuf>,
) -> anyhow::Result<Option<InstallResult>>
where
    B::Error: Send + Sync + std::error::Error + 'static,
{
    let platform_info = PlatformInfo::detect();
    let mut state = WizardState::new(platform_info, version, install_dir);

    let mut step_states = StepStates {
        components: ComponentsStepState::new(&state),
        server: ServerStepState::new(&state),
        embeddings: EmbeddingsStepState::new(),
        security: SecurityStepState::new(),
        system: SystemStepState::new(&state),
        advanced: AdvancedStepState::new(),
        summary: SummaryStepState::new(),
    };

    loop {
        // Draw frame.
        terminal.draw(|f| {
            let area = f.area();

            // Outer layout: title / step-indicator / content / nav-bar
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // title
                    Constraint::Length(2), // step indicator
                    Constraint::Min(10),   // step content
                    Constraint::Length(2), // nav bar
                ])
                .split(area);

            draw_title(f, chunks[0]);
            draw_step_indicator(f, chunks[1], &state.steps, state.current_step);

            // Delegate content rendering to the active step.
            match state.current_step {
                WizardStep::Components => {
                    draw_components_step(f, chunks[2], &state, &step_states.components);
                }
                WizardStep::ServerConfig => {
                    draw_server_step(f, chunks[2], &state, &step_states.server);
                }
                WizardStep::Embeddings => {
                    draw_embeddings_step(f, chunks[2], &state, &step_states.embeddings);
                }
                WizardStep::Security => {
                    draw_security_step(f, chunks[2], &state, &step_states.security);
                }
                WizardStep::SystemIntegration => {
                    draw_system_step(f, chunks[2], &state, &step_states.system);
                }
                WizardStep::Advanced => {
                    draw_advanced_step(f, chunks[2], &state, &step_states.advanced);
                }
                WizardStep::Summary => {
                    draw_summary_step(f, chunks[2], &state, &step_states.summary);
                }
            }

            draw_navigation_bar(f, chunks[3], !state.is_first_step());

            if state.show_quit_confirm {
                draw_quit_confirm(f, area);
            }
        })?;

        // Handle quit-confirm dialog separately.
        if state.show_quit_confirm {
            if event::poll(std::time::Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                return Ok(None);
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                state.show_quit_confirm = false;
                            }
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }

        // Poll for the next key event.
        if !event::poll(std::time::Duration::from_millis(50))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Ctrl+C / Ctrl+Q always quit immediately.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
        {
            state.show_quit_confirm = true;
            continue;
        }

        // Dispatch to active step handler.
        let result = match state.current_step {
            WizardStep::Components => {
                handle_components_input(key, &mut state, &mut step_states.components)
            }
            WizardStep::ServerConfig => {
                handle_server_input(key, &mut state, &mut step_states.server)
            }
            WizardStep::Embeddings => {
                handle_embeddings_input(key, &mut state, &mut step_states.embeddings)
            }
            WizardStep::Security => {
                handle_security_input(key, &mut state, &mut step_states.security)
            }
            WizardStep::SystemIntegration => {
                handle_system_input(key, &mut state, &mut step_states.system)
            }
            WizardStep::Advanced => {
                handle_advanced_input(key, &mut state, &mut step_states.advanced)
            }
            WizardStep::Summary => {
                handle_summary_input(key, &mut state, &mut step_states.summary).await
            }
        };

        // Handle navigation results.
        match result {
            StepResult::Continue => {}
            StepResult::Next => {
                if state.current_step == WizardStep::Summary {
                    // Summary confirmed and install ran -- return the result.
                    if let Some(ref r) = step_states.summary.install_result {
                        return Ok(Some(r.clone()));
                    }
                } else {
                    // After components step, rebuild step list and re-init dependent states.
                    if state.current_step == WizardStep::Components {
                        step_states.server = ServerStepState::new(&state);
                        step_states.system = SystemStepState::new(&state);
                        state.rebuild_steps();
                    }
                    state.next_step();
                }
            }
            StepResult::Back => {
                state.prev_step();
            }
            StepResult::Quit => {
                state.show_quit_confirm = true;
            }
        }
    }
}
