//! Security configuration step for the Kleos installer wizard.
//!
//! Shows all auto-generated security keys (encryption key, API key pepper,
//! initial API key, HMAC secret) with options to regenerate each key or paste
//! a custom value. Also includes an open-access toggle with a warning.

use crossterm::event::{KeyCode, KeyEvent};
use kleos_install_core::security;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::tui::{COLOR_ACTIVE, COLOR_DIM, COLOR_ERROR, COLOR_WARN};
use crate::types::StepResult;
use crate::wizard::WizardState;

/// Total number of navigable items (4 keys + open_access toggle = 5).
const ITEM_COUNT: usize = 5;
/// Item index for the open-access toggle.
const ITEM_OPEN_ACCESS: usize = 4;

/// Local UI state for the security configuration step.
pub struct SecurityStepState {
    /// Index of the currently focused item in the list (0-4).
    pub focused_index: usize,
    /// When `Some(i)`, field `i` is being edited with a custom value.
    pub editing_field: Option<usize>,
    /// Buffer for custom value being typed when editing a field.
    pub edit_buffer: String,
    /// Cursor position within `edit_buffer`.
    pub edit_cursor: usize,
}

impl SecurityStepState {
    /// Create the default security step state with no editing active.
    pub fn new() -> Self {
        SecurityStepState {
            focused_index: 0,
            editing_field: None,
            edit_buffer: String::new(),
            edit_cursor: 0,
        }
    }
}

/// Labels for the four key fields, in display order.
static KEY_LABELS: &[&str] = &[
    "Encryption key (SQLCipher)",
    "API key pepper (hash salt)",
    "Initial API key",
    "HMAC secret",
];

/// Draw the security configuration step into `area`.
pub fn draw_security_step(
    f: &mut Frame,
    area: Rect,
    state: &WizardState,
    step_state: &SecurityStepState,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Min(10),   // key list
            Constraint::Length(5), // open-access toggle + warning
        ])
        .split(area);

    draw_key_list(f, chunks[0], state, step_state);
    draw_open_access(f, chunks[1], state, step_state);
}

/// Render the four key rows.
fn draw_key_list(f: &mut Frame, area: Rect, state: &WizardState, step_state: &SecurityStepState) {
    let sc = &state.security_config;
    let key_values = [
        sc.encryption_key.as_str(),
        sc.api_key_pepper.as_str(),
        sc.initial_api_key.as_str(),
        sc.hmac_secret.as_str(),
    ];

    let mut items: Vec<ListItem> = Vec::new();

    for (i, (label, value)) in KEY_LABELS.iter().zip(key_values.iter()).enumerate() {
        let is_focused = step_state.focused_index == i;
        let is_editing = step_state.editing_field == Some(i);

        let label_style = if is_focused {
            Style::default()
                .fg(COLOR_ACTIVE)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_DIM)
        };

        let display_value = if is_editing {
            step_state.edit_buffer.as_str()
        } else {
            // Show first 16 chars + "..." + last 8 chars for long keys.
            value
        };

        let value_style = if is_editing {
            Style::default().fg(COLOR_WARN)
        } else {
            Style::default().fg(ratatui::style::Color::White)
        };

        let hint = if is_focused && !is_editing {
            "  [r] Regen  [e] Edit custom"
        } else if is_editing {
            "  [Enter] Accept  [Esc] Cancel"
        } else {
            ""
        };

        items.push(ListItem::new(vec![
            Line::from(vec![
                Span::raw(if is_focused { "> " } else { "  " }),
                Span::styled(*label, label_style),
                Span::styled(hint, Style::default().fg(COLOR_DIM)),
            ]),
            Line::from(vec![
                Span::raw("    "),
                Span::styled(truncate_key(display_value), value_style),
            ]),
        ]));
    }

    let title = " Security Keys (auto-generated) ";
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(COLOR_DIM)),
    );
    f.render_widget(list, area);
}

/// Render the open-access toggle with a warning message.
fn draw_open_access(
    f: &mut Frame,
    area: Rect,
    state: &WizardState,
    step_state: &SecurityStepState,
) {
    let is_focused = step_state.focused_index == ITEM_OPEN_ACCESS;
    let is_on = state.security_config.open_access;

    let toggle_text = if is_on { "[ON]  " } else { "[off] " };
    let toggle_style = if is_on {
        Style::default()
            .fg(COLOR_ERROR)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    let warning = if is_on {
        " WARNING: Open access disables API-key authentication. Do not use in production."
    } else {
        " Authentication required. Recommended for production deployments."
    };

    let lines = vec![
        Line::from(vec![
            Span::raw(if is_focused { "> " } else { "  " }),
            Span::styled(toggle_text, toggle_style),
            Span::styled(
                "Open access (no API key required)",
                Style::default().fg(COLOR_WARN),
            ),
        ]),
        Line::from(vec![
            Span::raw("    "),
            Span::styled(
                warning,
                if is_on {
                    Style::default().fg(COLOR_ERROR)
                } else {
                    Style::default().fg(COLOR_DIM)
                },
            ),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(if is_focused {
            Style::default().fg(COLOR_ACTIVE)
        } else {
            Style::default().fg(COLOR_DIM)
        })
        .title(" Access Control ");

    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

/// Byte offset of the `char_pos`-th character in `s` (its length if past the
/// end). Used to translate a char-based cursor into the byte index that
/// `String::remove`/`insert` require.
fn char_byte_idx(s: &str, char_pos: usize) -> usize {
    s.char_indices()
        .nth(char_pos)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Truncate a long key string for display, showing the first 20 and last 8 chars.
fn truncate_key(key: &str) -> String {
    let n = key.chars().count();
    if n <= 32 {
        key.to_string()
    } else {
        // Operate on chars, not byte slices: a pasted multibyte key panics on
        // `&key[..20]` when the index lands mid-char.
        let head: String = key.chars().take(20).collect();
        let tail: String = key.chars().skip(n - 8).collect();
        format!("{head}...{tail}")
    }
}

/// Handle a key event for the security configuration step.
pub fn handle_security_input(
    key: KeyEvent,
    state: &mut WizardState,
    step_state: &mut SecurityStepState,
) -> StepResult {
    // Handle editing mode first.
    if let Some(field_idx) = step_state.editing_field {
        match key.code {
            KeyCode::Enter => {
                // Apply custom value.
                let val = step_state.edit_buffer.trim().to_string();
                if !val.is_empty() {
                    apply_custom_key(state, field_idx, val);
                }
                step_state.editing_field = None;
                step_state.edit_buffer.clear();
                step_state.edit_cursor = 0;
            }
            KeyCode::Esc => {
                step_state.editing_field = None;
                step_state.edit_buffer.clear();
                step_state.edit_cursor = 0;
            }
            KeyCode::Backspace if step_state.edit_cursor > 0 => {
                // edit_cursor counts chars; String::remove/insert want byte
                // indices. Convert so a multibyte char in the buffer cannot
                // panic or corrupt the string.
                let byte = char_byte_idx(&step_state.edit_buffer, step_state.edit_cursor - 1);
                step_state.edit_buffer.remove(byte);
                step_state.edit_cursor -= 1;
            }
            KeyCode::Char(c) => {
                let byte = char_byte_idx(&step_state.edit_buffer, step_state.edit_cursor);
                step_state.edit_buffer.insert(byte, c);
                step_state.edit_cursor += 1;
            }
            _ => {}
        }
        return StepResult::Continue;
    }

    // Normal navigation mode.
    match key.code {
        KeyCode::Down | KeyCode::Tab
            if step_state.focused_index + 1 < ITEM_COUNT => {
                step_state.focused_index += 1;
            }
        KeyCode::Up | KeyCode::BackTab
            if step_state.focused_index > 0 => {
                step_state.focused_index -= 1;
            }
        KeyCode::Enter
            if step_state.focused_index == ITEM_COUNT - 1 => {
                // Last item -- also the toggle; Enter on last item = advance.
                return StepResult::Next;
            }
        KeyCode::Esc | KeyCode::Backspace => {
            return StepResult::Back;
        }
        KeyCode::Char('q') => {
            return StepResult::Quit;
        }
        KeyCode::Char('r')
            // Regenerate focused key.
            if step_state.focused_index < KEY_LABELS.len() => {
                regenerate_key(state, step_state.focused_index);
            }
        KeyCode::Char('e')
            // Enter edit mode for focused key.
            if step_state.focused_index < KEY_LABELS.len() => {
                step_state.editing_field = Some(step_state.focused_index);
                step_state.edit_buffer.clear();
                step_state.edit_cursor = 0;
            }
        KeyCode::Char(' ')
            // Toggle open access.
            if step_state.focused_index == ITEM_OPEN_ACCESS => {
                state.security_config.open_access = !state.security_config.open_access;
            }
        _ => {}
    }

    StepResult::Continue
}

/// Regenerate the security key at the given index.
fn regenerate_key(state: &mut WizardState, index: usize) {
    match index {
        0 => state.security_config.encryption_key = security::generate_encryption_key(),
        1 => state.security_config.api_key_pepper = security::generate_api_key_pepper(),
        2 => state.security_config.initial_api_key = security::generate_api_key(),
        3 => state.security_config.hmac_secret = security::generate_hmac_secret(),
        _ => {}
    }
}

/// Apply a custom key value at the given index.
fn apply_custom_key(state: &mut WizardState, index: usize, val: String) {
    match index {
        0 => state.security_config.encryption_key = val,
        1 => state.security_config.api_key_pepper = val,
        2 => state.security_config.initial_api_key = val,
        3 => state.security_config.hmac_secret = val,
        _ => {}
    }
}
