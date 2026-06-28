//! Entry point for the `kleos-install` TUI installer binary.
//!
//! Parses CLI arguments, then either runs the interactive TUI wizard or
//! falls through to non-interactive mode. On any panic during TUI operation
//! the raw terminal is restored before the process exits so the user's shell
//! is not left in an unusable state.

mod noninteractive;
mod steps;
mod tui;
mod types;
mod wizard;

use std::io;
use std::panic;

use clap::Parser;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tracing_subscriber::EnvFilter;

/// Interactive TUI installer for Kleos.
#[derive(Parser)]
#[command(name = "kleos-install", about = "Interactive installer for Kleos")]
struct Args {
    /// Run in non-interactive mode with defaults.
    #[arg(long)]
    non_interactive: bool,

    /// Kleos version to install (default: latest).
    #[arg(long)]
    version: Option<String>,

    /// Installation directory override.
    #[arg(long)]
    install_dir: Option<std::path::PathBuf>,

    /// Installation profile for non-interactive mode: server, agent-host, full.
    #[arg(long, default_value = "server")]
    profile: String,

    /// Override any config field (non-interactive): --set field=value. Repeatable;
    /// use dotted keys for nested fields, e.g. --set backup_enabled=true
    /// --set eidolon.enabled=true.
    #[arg(long = "set", value_name = "FIELD=VALUE")]
    set: Vec<String>,

    /// Append a raw KEY=VALUE line to the generated .env (non-interactive).
    /// Repeatable; for env-only settings like KLEOS_GUI_PASSWORD.
    #[arg(long = "env", value_name = "KEY=VALUE")]
    env: Vec<String>,

    /// Seed configuration from an existing kleos.toml before applying overrides
    /// (non-interactive). The imported file is authoritative for config fields.
    #[arg(long, value_name = "PATH")]
    config_file: Option<std::path::PathBuf>,

    /// Enable anonymous read-only access (writes the three env vars the server
    /// requires together). Non-interactive.
    #[arg(long)]
    open_access: bool,

    /// Comma-separated allowed CORS origins (non-interactive).
    #[arg(long, value_name = "ORIGINS")]
    cors: Option<String>,

    /// Set the GUI password, which enables the web GUI (non-interactive).
    #[arg(long, value_name = "PASSWORD")]
    gui_password: Option<String>,
}

/// Initialize the tracing subscriber for diagnostic output.
///
/// In non-interactive mode output goes to stderr. In TUI mode tracing is
/// suppressed so it does not corrupt the alternate screen.
fn init_tracing(tui_mode: bool) {
    if tui_mode {
        // Suppress tracing output so it does not bleed into the TUI.
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new("off"))
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .try_init();
    }
}

/// Install a panic hook that restores the terminal before printing the panic message.
///
/// Without this, a panic inside the TUI event loop leaves the terminal in raw
/// mode and the alternate screen active, making the shell unusable.
fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Best-effort terminal restore -- ignore errors.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        default_hook(info);
    }));
}

/// Parse CLI args and dispatch to the non-interactive runner or the TUI wizard.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.non_interactive {
        init_tracing(false);
        let cli = noninteractive::CliConfig {
            set: args.set,
            env: args.env,
            config_file: args.config_file,
            open_access: args.open_access,
            cors: args.cors,
            gui_password: args.gui_password,
        };
        noninteractive::run(args.version, args.install_dir, &args.profile, cli).await
    } else {
        init_tracing(true);
        install_panic_hook();

        // Set up the crossterm alternate-screen terminal.
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = wizard::run_wizard(&mut terminal, args.version, args.install_dir).await;

        // Always restore the terminal even if the wizard returned an error.
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        match result? {
            Some(result) => {
                println!("Installation complete.");
                if let Some(url) = &result.server_url {
                    println!("Server URL: {url}");
                }
                println!("API key: {}", result.api_key);
            }
            None => {
                println!("Installation cancelled.");
            }
        }

        Ok(())
    }
}
