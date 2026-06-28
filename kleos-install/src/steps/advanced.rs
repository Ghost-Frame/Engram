//! Advanced (expert) configuration step for the Kleos installer wizard.
//!
//! Exposes the commonly-flipped server toggles -- background workers, retrieval
//! channels, backups, rate limiting, open access -- as a navigable list so a
//! user does not have to hand-edit `kleos.toml` after install. Anything not
//! listed here is still settable: the generated `kleos.toml` already contains
//! every field at its default, and the non-interactive CLI accepts
//! `--set field=value` for the full surface. Curated values that the user
//! changes are emitted as overrides; unchanged ones are left at their default.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use kleos_install_core::config::ConfigOverrides;

use crate::tui::{COLOR_ACTIVE, COLOR_DIM};
use crate::types::{InputField, StepResult};
use crate::wizard::WizardState;

/// Where an advanced item's value is routed when the plan is assembled.
enum Sink {
    /// Override a `kleos_config::Config` field via `toml_overrides` using this
    /// exact (possibly dotted) key.
    TomlField(&'static str),
    /// Set the open-access security flag (which expands to the three env vars the
    /// server requires together).
    OpenAccess,
    /// Append an env-only `KEY=VALUE` line to `.env` using this var name.
    Env(&'static str),
}

/// The editable value of an advanced item: a boolean toggle or a text field.
enum AdvValue {
    /// A boolean toggle with its current and original (default) state.
    Bool { on: bool, default_on: bool },
    /// A free-text value with its original (default) string for change detection.
    Text { field: InputField, default: String },
}

/// One row in the advanced step: a labelled, editable setting with a sink.
struct AdvancedItem {
    /// Display label shown in the list.
    label: &'static str,
    /// One-line help shown when the row is selected.
    help: &'static str,
    /// Destination the value is written to when assembling the plan.
    sink: Sink,
    /// The current editable value.
    value: AdvValue,
}

/// Local UI state for the advanced configuration step.
pub struct AdvancedStepState {
    /// All advanced setting rows.
    items: Vec<AdvancedItem>,
    /// Index of the currently selected row.
    selected: usize,
}

/// Build a boolean toggle item.
fn toggle(
    label: &'static str,
    help: &'static str,
    key: &'static str,
    default_on: bool,
) -> AdvancedItem {
    AdvancedItem {
        label,
        help,
        sink: Sink::TomlField(key),
        value: AdvValue::Bool {
            on: default_on,
            default_on,
        },
    }
}

/// Build a free-text item with a default value.
fn text(
    label: &'static str,
    help: &'static str,
    sink: Sink,
    default: &str,
    placeholder: &str,
) -> AdvancedItem {
    AdvancedItem {
        label,
        help,
        sink,
        value: AdvValue::Text {
            field: InputField::with_value(label, default.to_string(), placeholder.to_string()),
            default: default.to_string(),
        },
    }
}

/// Construction and selection helpers for the advanced step.
impl AdvancedStepState {
    /// Build the advanced step with the curated set of toggles and values,
    /// seeded from the server defaults.
    pub fn new() -> Self {
        let items = vec![
            // Access control.
            AdvancedItem {
                label: "Open access (anonymous read-only)",
                help: "Allow unauthenticated read-only access. Forces single-tenant mode.",
                sink: Sink::OpenAccess,
                value: AdvValue::Bool {
                    on: false,
                    default_on: false,
                },
            },
            // Background workers.
            toggle(
                "Auto-backup",
                "Periodically snapshot the database to the backup directory.",
                "backup_enabled",
                false,
            ),
            toggle(
                "Dreamer task",
                "Background consolidation / maintenance loop.",
                "dreamer_enabled",
                true,
            ),
            toggle(
                "Associative auto-linker",
                "Dreamer links unlinked memories to nearest neighbours each cycle.",
                "auto_link_enabled",
                true,
            ),
            toggle(
                "PageRank scoring",
                "Maintain PageRank scores used to boost well-connected memories.",
                "pagerank_enabled",
                true,
            ),
            toggle(
                "Skill evolution",
                "Autonomous skill fix / capture / derive inside the dreamer tick.",
                "skill_evolution_enabled",
                true,
            ),
            toggle(
                "Thymus auto-evaluation",
                "Evaluate sessions automatically when they end.",
                "thymus_autoeval_enabled",
                true,
            ),
            toggle(
                "Community detection",
                "Scheduled Louvain job that maintains the community retrieval channel.",
                "community_detection_enabled",
                false,
            ),
            toggle(
                "Consolidation endpoints",
                "Enable consolidation (merges memories; can degrade search). Off by default.",
                "consolidation_enabled",
                false,
            ),
            // Retrieval tuning.
            toggle(
                "Facts retrieval channel",
                "Add structured_facts as an RRF retrieval channel (changes ranking).",
                "facts_channel_enabled",
                false,
            ),
            toggle(
                "Chunk-level vector search",
                "Search at chunk granularity in addition to whole memories.",
                "use_chunk_vector_search",
                false,
            ),
            // Embedding.
            toggle(
                "Offline-only embedding",
                "Refuse to download model weights at boot (air-gapped deployments).",
                "embedding_offline_only",
                false,
            ),
            // Values.
            text(
                "Web search (SearXNG) URL",
                "Base URL of the SearXNG instance backing /search/web.",
                Sink::TomlField("web_search_url"),
                "http://127.0.0.1:8888",
                "http://127.0.0.1:8888",
            ),
            text(
                "Pre-auth per-IP rate limit (rpm)",
                "Requests per minute allowed before authentication, per source IP.",
                Sink::TomlField("preauth_ip_rpm"),
                "60",
                "60",
            ),
            text(
                "GUI password",
                "Any non-empty value enables the web GUI (KLEOS_GUI_PASSWORD).",
                Sink::Env("KLEOS_GUI_PASSWORD"),
                "",
                "(leave blank to keep the GUI disabled)",
            ),
        ];

        AdvancedStepState { items, selected: 0 }
    }

    /// Whether the currently selected row is a text field (so typed characters
    /// are routed to it).
    fn selected_is_text(&self) -> bool {
        matches!(self.items[self.selected].value, AdvValue::Text { .. })
    }
}

/// Default construction for the advanced step.
impl Default for AdvancedStepState {
    /// Delegate to [`AdvancedStepState::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Draw the advanced configuration step into `area`.
pub fn draw_advanced_step(
    f: &mut Frame,
    area: Rect,
    _state: &WizardState,
    step_state: &AdvancedStepState,
) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        "Optional advanced toggles -- every other setting lives in the generated kleos.toml.",
        Style::default().fg(COLOR_DIM),
    )));
    lines.push(Line::from(""));

    for (i, item) in step_state.items.iter().enumerate() {
        let selected = i == step_state.selected;
        let marker = if selected { "> " } else { "  " };
        let rendered = match &item.value {
            AdvValue::Bool { on, .. } => {
                format!("{marker}[{}] {}", if *on { "x" } else { " " }, item.label)
            }
            AdvValue::Text { field, .. } => {
                let shown = if field.value.is_empty() {
                    field.placeholder.clone()
                } else {
                    field.value.clone()
                };
                format!("{marker}{}: {}", item.label, shown)
            }
        };
        let style = if selected {
            Style::default()
                .fg(COLOR_ACTIVE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(rendered, style)));
    }

    // Help text for the selected row.
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        step_state.items[step_state.selected].help,
        Style::default().fg(COLOR_DIM),
    )));
    lines.push(Line::from(Span::styled(
        "Up/Down select - Space toggle - type to edit value - Enter continue - Esc back",
        Style::default().fg(COLOR_DIM),
    )));

    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Advanced settings "),
    );
    f.render_widget(p, area);
}

/// Handle a key event for the advanced configuration step.
pub fn handle_advanced_input(
    key: KeyEvent,
    state: &mut WizardState,
    step_state: &mut AdvancedStepState,
) -> StepResult {
    let count = step_state.items.len();
    match key.code {
        KeyCode::Up => {
            if step_state.selected > 0 {
                step_state.selected -= 1;
            }
        }
        KeyCode::Down => {
            if step_state.selected + 1 < count {
                step_state.selected += 1;
            }
        }
        KeyCode::Char(' ') => {
            // Space toggles a boolean; for a text field it is a literal space.
            match &mut step_state.items[step_state.selected].value {
                AdvValue::Bool { on, .. } => *on = !*on,
                AdvValue::Text { field, .. } => field.insert_char(' '),
            }
        }
        KeyCode::Enter => {
            apply_to_state(state, step_state);
            return StepResult::Next;
        }
        KeyCode::Esc => return StepResult::Back,
        KeyCode::Backspace => {
            if let AdvValue::Text { field, .. } = &mut step_state.items[step_state.selected].value {
                field.delete_char_before();
            } else {
                return StepResult::Back;
            }
        }
        KeyCode::Char(c) if step_state.selected_is_text() => {
            if let AdvValue::Text { field, .. } = &mut step_state.items[step_state.selected].value {
                field.insert_char(c);
            }
        }
        _ => {}
    }
    StepResult::Continue
}

/// Fold the advanced rows into `WizardState`: changed config-field values become
/// `toml_overrides`, open access flips the security flag, and non-empty env
/// values become extra `.env` entries. Unchanged defaults are not emitted.
fn apply_to_state(state: &mut WizardState, step_state: &AdvancedStepState) {
    let mut overrides = ConfigOverrides::default();

    for item in &step_state.items {
        match (&item.sink, &item.value) {
            (Sink::OpenAccess, AdvValue::Bool { on, .. }) => {
                state.security_config.open_access = *on;
            }
            (Sink::TomlField(key), AdvValue::Bool { on, default_on }) => {
                if on != default_on {
                    overrides
                        .toml_overrides
                        .push((key.to_string(), on.to_string()));
                }
            }
            (Sink::TomlField(key), AdvValue::Text { field, default }) => {
                let value = field.value.trim();
                if !value.is_empty() && value != default {
                    overrides
                        .toml_overrides
                        .push((key.to_string(), value.to_string()));
                }
            }
            (Sink::Env(var), AdvValue::Text { field, .. }) => {
                let value = field.value.trim();
                if !value.is_empty() {
                    overrides
                        .extra_env
                        .push((var.to_string(), value.to_string()));
                }
            }
            // Remaining combinations (e.g. a boolean env var, or open-access as
            // text) are not produced by any current item.
            _ => {}
        }
    }

    state.advanced_overrides = overrides;
}
