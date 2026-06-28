//! System integration step for the Kleos installer wizard.
//!
//! Lets the user choose which service manager should manage the Kleos server
//! process. Only options available on the current platform are shown.
//! A preview of the generated unit file or plist is shown in a scrollable block.

use crossterm::event::{KeyCode, KeyEvent};
use kleos_install_core::system::SystemIntegration;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::{COLOR_ACTIVE, COLOR_DIM};
use crate::types::StepResult;
use crate::wizard::WizardState;

/// Which system integration option is available (depends on platform).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemOption {
    /// Install a systemd user service unit.
    Systemd,
    /// Install a launchd user agent plist.
    Launchd,
    /// Do not integrate with any service manager.
    None,
}

/// Local UI state for the system integration step.
pub struct SystemStepState {
    /// All options available on the current platform.
    pub options: Vec<SystemOption>,
    /// Index of the currently selected option.
    pub selected: usize,
    /// Whether auto-start is enabled for the selected service manager.
    pub auto_start: bool,
    /// Focus: 0 = options list, 1 = auto-start toggle.
    pub focused_item: usize,
    /// Scroll offset for the preview pane.
    pub preview_scroll: u16,
}

/// Construction and state transitions for the system-integration wizard step.
impl SystemStepState {
    /// Build the initial system step state from the current wizard state.
    ///
    /// Options are filtered to those available on the detected platform.
    /// The initial selection mirrors the auto-detected system integration.
    pub fn new(state: &WizardState) -> Self {
        let mut options = Vec::new();
        if state.platform_info.has_systemd {
            options.push(SystemOption::Systemd);
        }
        if state.platform_info.has_launchd {
            options.push(SystemOption::Launchd);
        }
        options.push(SystemOption::None);

        // Match existing system_integration to select the right option.
        let selected = match &state.system_integration {
            SystemIntegration::Systemd { .. } => options
                .iter()
                .position(|o| *o == SystemOption::Systemd)
                .unwrap_or(0),
            SystemIntegration::Launchd { .. } => options
                .iter()
                .position(|o| *o == SystemOption::Launchd)
                .unwrap_or(0),
            SystemIntegration::None | SystemIntegration::WindowsService => options
                .iter()
                .position(|o| *o == SystemOption::None)
                .unwrap_or(0),
        };

        let auto_start = match &state.system_integration {
            SystemIntegration::Systemd { auto_start } => *auto_start,
            SystemIntegration::Launchd { auto_start } => *auto_start,
            _ => true,
        };

        SystemStepState {
            options,
            selected,
            auto_start,
            focused_item: 0,
            preview_scroll: 0,
        }
    }

    /// Return the system integration value corresponding to the current selection.
    pub fn to_integration(&self) -> SystemIntegration {
        let option = self
            .options
            .get(self.selected)
            .copied()
            .unwrap_or(SystemOption::None);
        match option {
            SystemOption::Systemd => SystemIntegration::Systemd {
                auto_start: self.auto_start,
            },
            SystemOption::Launchd => SystemIntegration::Launchd {
                auto_start: self.auto_start,
            },
            SystemOption::None => SystemIntegration::None,
        }
    }
}

/// Draw the system integration step into `area`.
pub fn draw_system_step(
    f: &mut Frame,
    area: Rect,
    state: &WizardState,
    step_state: &SystemStepState,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(step_state.options.len() as u16 + 2), // options
            Constraint::Length(3),                                   // auto-start toggle
            Constraint::Min(5),                                      // preview
        ])
        .split(area);

    draw_options(f, chunks[0], step_state);
    draw_auto_start(f, chunks[1], step_state);
    draw_preview(f, chunks[2], state, step_state);
}

/// Render the service manager option radio buttons.
fn draw_options(f: &mut Frame, area: Rect, step_state: &SystemStepState) {
    let is_focused = step_state.focused_item == 0;
    let border_style = if is_focused {
        Style::default().fg(COLOR_ACTIVE)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    let lines: Vec<Line> = step_state
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let is_selected = step_state.selected == i;
            let label = match opt {
                SystemOption::Systemd => "systemd user service",
                SystemOption::Launchd => "launchd user agent",
                SystemOption::None => "None (start manually)",
            };
            Line::from(vec![
                Span::raw(if is_selected { " (o) " } else { " ( ) " }),
                Span::styled(
                    label,
                    if is_selected {
                        Style::default()
                            .fg(COLOR_ACTIVE)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(ratatui::style::Color::White)
                    },
                ),
            ])
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" Service Manager ");
    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

/// Render the auto-start toggle.
fn draw_auto_start(f: &mut Frame, area: Rect, step_state: &SystemStepState) {
    let is_focused = step_state.focused_item == 1;
    let border_style = if is_focused {
        Style::default().fg(COLOR_ACTIVE)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    let toggle = if step_state.auto_start { "[x]" } else { "[ ]" };
    let p = Paragraph::new(Line::from(vec![
        Span::raw(format!(" {} ", toggle)),
        Span::raw("Enable and start automatically on login"),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(" Auto-start "),
    );
    f.render_widget(p, area);
}

/// Render the generated unit file / plist preview.
fn draw_preview(f: &mut Frame, area: Rect, state: &WizardState, step_state: &SystemStepState) {
    let integration = step_state.to_integration();
    let preview = match &integration {
        SystemIntegration::Systemd { .. } => kleos_install_core::system::generate_systemd_unit(
            &build_installer_config(state),
            &state.install_dir,
            &state.config_dir,
        ),
        SystemIntegration::Launchd { .. } => kleos_install_core::system::generate_launchd_plist(
            &build_installer_config(state),
            &state.install_dir,
            &state.config_dir,
        )
        .unwrap_or_else(|e| format!("Could not render plist preview: {e}")),
        _ => String::from("No service file will be generated."),
    };

    let p = Paragraph::new(preview)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_DIM))
                .title(" Preview (generated unit file) "),
        )
        .scroll((step_state.preview_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

/// Build a minimal `InstallerConfig` from wizard state for unit file generation.
fn build_installer_config(state: &WizardState) -> kleos_install_core::config::InstallerConfig {
    let has_server = state
        .selected_components
        .iter()
        .any(|c| c == "kleos-server");
    kleos_install_core::config::InstallerConfig {
        server: if has_server {
            Some(state.server_config.clone())
        } else {
            None
        },
        embedding: state.embedding_config.clone(),
        reranker: state.reranker_config.clone(),
        security: state.security_config.clone(),
        overrides: kleos_install_core::config::ConfigOverrides::default(),
    }
}

/// Handle a key event for the system integration step.
pub fn handle_system_input(
    key: KeyEvent,
    state: &mut WizardState,
    step_state: &mut SystemStepState,
) -> StepResult {
    match key.code {
        KeyCode::Enter => {
            // Apply selection and advance.
            state.system_integration = step_state.to_integration();
            return StepResult::Next;
        }
        KeyCode::Esc | KeyCode::Backspace => {
            return StepResult::Back;
        }
        KeyCode::Char('q') => {
            return StepResult::Quit;
        }
        KeyCode::Tab | KeyCode::Down => {
            if step_state.focused_item == 0 {
                if step_state.selected + 1 < step_state.options.len() {
                    step_state.selected += 1;
                } else {
                    step_state.focused_item = 1;
                }
            } else {
                step_state.focused_item = 0;
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if step_state.focused_item == 1 {
                step_state.focused_item = 0;
            } else if step_state.selected > 0 {
                step_state.selected -= 1;
            }
        }
        KeyCode::Char(' ') if step_state.focused_item == 1 => {
            step_state.auto_start = !step_state.auto_start;
        }
        KeyCode::PageDown => {
            step_state.preview_scroll = step_state.preview_scroll.saturating_add(3);
        }
        KeyCode::PageUp => {
            step_state.preview_scroll = step_state.preview_scroll.saturating_sub(3);
        }
        _ => {}
    }

    StepResult::Continue
}
