use chrono::{DateTime, Utc};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph},
};
use serde::{Deserialize, Serialize};
use std::io;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "engram-approval-tui")]
#[command(about = "Terminal UI for human approval workflow")]
struct Args {
    /// Engram server URL
    #[arg(short, long, env = "ENGRAM_URL", default_value = "http://localhost:4200")]
    url: String,

    /// API key for authentication
    #[arg(short = 'k', long, env = "ENGRAM_API_KEY")]
    api_key: String,

    /// Poll interval in milliseconds
    #[arg(short, long, default_value = "1000")]
    poll_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct Approval {
    id: String,
    action: String,
    context: Option<String>,
    requester: String,
    status: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    seconds_remaining: i64,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PendingResponse {
    approvals: Vec<Approval>,
    count: usize,
    expired_count: u64,
}

#[derive(Debug, Serialize)]
struct DecideRequest {
    decision: String,
    decided_by: Option<String>,
    reason: Option<String>,
}

struct App {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    approvals: Vec<Approval>,
    selected: usize,
    list_state: ListState,
    last_error: Option<String>,
    detail_mode: bool,
}

impl App {
    fn new(url: String, api_key: String) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));
        Self {
            client: reqwest::Client::new(),
            base_url: url,
            api_key,
            approvals: Vec::new(),
            selected: 0,
            list_state,
            last_error: None,
            detail_mode: false,
        }
    }

    async fn fetch_pending(&mut self) {
        let url = format!("{}/approvals/pending", self.base_url);
        match self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<PendingResponse>().await {
                        Ok(data) => {
                            self.approvals = data.approvals;
                            self.last_error = None;
                            // Clamp selection to valid range
                            if !self.approvals.is_empty() && self.selected >= self.approvals.len() {
                                self.selected = self.approvals.len() - 1;
                            }
                            self.list_state.select(if self.approvals.is_empty() {
                                None
                            } else {
                                Some(self.selected)
                            });
                        }
                        Err(e) => {
                            self.last_error = Some(format!("Parse error: {}", e));
                        }
                    }
                } else {
                    self.last_error = Some(format!("HTTP {}", resp.status()));
                }
            }
            Err(e) => {
                self.last_error = Some(format!("Connection error: {}", e));
            }
        }
    }

    async fn decide(&mut self, approved: bool) {
        if self.approvals.is_empty() {
            return;
        }

        let approval = &self.approvals[self.selected];
        let url = format!("{}/approvals/{}/decide", self.base_url, approval.id);
        let decision = if approved { "approved" } else { "denied" };

        let req = DecideRequest {
            decision: decision.to_string(),
            decided_by: Some("tui-operator".to_string()),
            reason: None,
        };

        match self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&req)
            .send()
            .await
        {
            Ok(resp) => {
                if resp.status().is_success() {
                    self.last_error = None;
                    // Refresh list
                    self.fetch_pending().await;
                } else {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    self.last_error = Some(format!("HTTP {}: {}", status, body));
                }
            }
            Err(e) => {
                self.last_error = Some(format!("Request error: {}", e));
            }
        }
    }

    fn select_next(&mut self) {
        if self.approvals.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.approvals.len();
        self.list_state.select(Some(self.selected));
    }

    fn select_prev(&mut self) {
        if self.approvals.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.approvals.len() - 1
        } else {
            self.selected - 1
        };
        self.list_state.select(Some(self.selected));
    }
}

fn ui(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title
            Constraint::Min(10),   // List
            Constraint::Length(3), // Help
        ])
        .split(frame.area());

    // Title bar
    let title = Paragraph::new(format!(
        " ENGRAM APPROVAL CONSOLE                                    Pending: {}",
        app.approvals.len()
    ))
    .style(Style::default().fg(Color::White).bg(Color::Blue).bold())
    .block(Block::default());
    frame.render_widget(title, chunks[0]);

    // Main content area
    if app.detail_mode && !app.approvals.is_empty() {
        // Detail view
        render_detail(frame, app, chunks[1]);
    } else {
        // List view
        render_list(frame, app, chunks[1]);
    }

    // Help bar
    let help_text = if app.detail_mode {
        " [a] Approve  [d] Deny  [Esc] Back  [q] Quit"
    } else {
        " [↑/↓] Select  [a] Approve  [d] Deny  [Enter] Details  [q] Quit"
    };
    let error_text = app
        .last_error
        .as_ref()
        .map(|e| format!("  ERROR: {}", e))
        .unwrap_or_default();

    let help = Paragraph::new(format!("{}{}", help_text, error_text))
        .style(if app.last_error.is_some() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Gray)
        })
        .block(Block::default().borders(Borders::TOP));
    frame.render_widget(help, chunks[2]);
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    if app.approvals.is_empty() {
        let empty = Paragraph::new("\n\n  No pending approvals.\n\n  Waiting for approval requests...")
            .style(Style::default().fg(Color::Gray))
            .block(Block::default().borders(Borders::ALL).title(" Pending Approvals "));
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = app
        .approvals
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let marker = if i == app.selected { ">" } else { " " };
            let time_bar = make_time_bar(a.seconds_remaining);
            ListItem::new(vec![
                Line::from(vec![
                    Span::raw(format!("{} [{}] ", marker, i + 1)),
                    Span::styled(&a.action, Style::default().fg(Color::Cyan)),
                    Span::raw(format!(" {}", time_bar)),
                    Span::styled(
                        format!(" {:>3}s", a.seconds_remaining),
                        if a.seconds_remaining < 30 {
                            Style::default().fg(Color::Red).bold()
                        } else if a.seconds_remaining < 60 {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default().fg(Color::Green)
                        },
                    ),
                ]),
                Line::from(vec![
                    Span::raw("     Requester: "),
                    Span::styled(&a.requester, Style::default().fg(Color::Gray)),
                ]),
            ])
            .style(if i == app.selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            })
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" Pending Approvals "))
        .highlight_style(Style::default());

    frame.render_stateful_widget(list, area, &mut app.list_state.clone());
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let approval = &app.approvals[app.selected];

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Timer
            Constraint::Min(5),    // Details
        ])
        .split(area);

    // Timer gauge
    let ratio = (approval.seconds_remaining as f64 / 120.0).clamp(0.0, 1.0);
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Time Remaining "))
        .gauge_style(if approval.seconds_remaining < 30 {
            Style::default().fg(Color::Red)
        } else if approval.seconds_remaining < 60 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Green)
        })
        .ratio(ratio)
        .label(format!("{}s", approval.seconds_remaining));
    frame.render_widget(gauge, chunks[0]);

    // Details
    let context_str = approval
        .context
        .as_ref()
        .map(|c| {
            // Pretty print JSON if possible
            serde_json::from_str::<serde_json::Value>(c)
                .map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| c.clone()))
                .unwrap_or_else(|_| c.clone())
        })
        .unwrap_or_else(|| "(none)".to_string());

    let detail_text = format!(
        "Action: {}\n\nRequester: {}\n\nContext:\n{}\n\nCreated: {}",
        approval.action,
        approval.requester,
        context_str,
        approval.created_at.format("%Y-%m-%d %H:%M:%S UTC")
    );

    let detail = Paragraph::new(detail_text)
        .block(Block::default().borders(Borders::ALL).title(" Details "))
        .wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(detail, chunks[1]);
}

fn make_time_bar(seconds: i64) -> String {
    let filled = ((seconds as f64 / 120.0) * 6.0).ceil() as usize;
    let filled = filled.min(6);
    let empty = 6 - filled;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(args.url, args.api_key);
    let poll_duration = Duration::from_millis(args.poll_ms);

    // Initial fetch
    app.fetch_pending().await;

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Poll for events with timeout
        if event::poll(poll_duration)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('a') => {
                            app.decide(true).await;
                        }
                        KeyCode::Char('d') => {
                            app.decide(false).await;
                        }
                        KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
                        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                        KeyCode::Enter => {
                            if !app.approvals.is_empty() {
                                app.detail_mode = true;
                            }
                        }
                        KeyCode::Esc => {
                            app.detail_mode = false;
                        }
                        _ => {}
                    }
                }
            }
        } else {
            // Timeout - refresh data
            app.fetch_pending().await;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
