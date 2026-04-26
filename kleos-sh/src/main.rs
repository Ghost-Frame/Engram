mod exec;
mod gate;
mod observe;

use clap::Parser;
use std::process;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "kleos-sh", about = "Universal shell gate for AI agents")]
struct Cli {
    #[arg(short = 'c', help = "Command to execute")]
    command: Option<String>,

    #[arg(long, help = "Gate-only mode: check and exit, do not execute")]
    gate_only: bool,

    #[arg(long, default_value = "claude-code", help = "Agent identity")]
    agent: String,

    #[arg(long, help = "Tool name for gate check (e.g. Bash, Write)")]
    tool_name: Option<String>,
}

fn resolve_api_key() -> Option<String> {
    if let Ok(key) = std::env::var("KLEOS_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    if let Ok(key) = std::env::var("EIDOLON_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    let output = std::process::Command::new("cred")
        .args(["get", "engram-rust", "claude-code-wsl", "--raw"])
        .output()
        .ok()?;
    if output.status.success() {
        let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !key.is_empty() {
            return Some(key);
        }
    }
    None
}

fn server_url() -> String {
    std::env::var("KLEOS_SERVER_URL")
        .or_else(|_| std::env::var("ENGRAM_EIDOLON_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:4200".to_string())
}

fn sidecar_url() -> String {
    std::env::var("KLEOS_SIDECAR_URL").unwrap_or_else(|_| "http://127.0.0.1:4201".to_string())
}

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(150))
        .redirect(reqwest::redirect::Policy::limited(1))
        .build()
        .expect("failed to build HTTP client")
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let command = match &cli.command {
        Some(cmd) => cmd.clone(),
        None => {
            let mut input = String::new();
            if std::io::Read::read_to_string(&mut std::io::stdin(), &mut input).is_err()
                || input.is_empty()
            {
                process::exit(0);
            }
            input
        }
    };

    if command.trim().is_empty() {
        process::exit(0);
    }

    if let Some(reason) = gate::check_offline(&command) {
        eprintln!("{}", reason);
        process::exit(2);
    }

    let api_key = resolve_api_key();
    let server = server_url();
    let sidecar = sidecar_url();
    let client = build_client();

    let req = gate::GateCheckRequest {
        command: command.clone(),
        agent: cli.agent.clone(),
        context: None,
        tool_name: cli.tool_name.clone().or_else(|| Some("Bash".to_string())),
    };

    let outcome = match &api_key {
        Some(key) => match gate::check_remote(&client, &server, key, &req).await {
            Ok(outcome) => outcome,
            Err(err) => {
                eprintln!("kleos-sh: gate unreachable ({}), failing open", err);
                gate::GateOutcome::Allow {
                    command: command.clone(),
                    enrichment: None,
                    gate_id: 0,
                }
            }
        },
        None => {
            eprintln!("kleos-sh: no API key available, failing open");
            gate::GateOutcome::Allow {
                command: command.clone(),
                enrichment: None,
                gate_id: 0,
            }
        }
    };

    match outcome {
        gate::GateOutcome::Deny { reason, .. } => {
            eprintln!("EIDOLON GATE DENIED: {}", reason);
            process::exit(2);
        }
        gate::GateOutcome::Allow {
            command: resolved_cmd,
            enrichment,
            gate_id,
        } => {
            if let Some(ctx) = &enrichment {
                eprintln!("[kleos enrichment] {}", ctx);
            }

            if cli.gate_only {
                process::exit(0);
            }

            let result = exec::run_command(&resolved_cmd).await;

            match result {
                Ok(res) => {
                    if let Some(key) = &api_key {
                        observe::fire_and_forget(
                            &client,
                            &sidecar,
                            key,
                            &cli.agent,
                            &resolved_cmd,
                            gate_id,
                            res.exit_code,
                        );
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                    process::exit(res.exit_code);
                }
                Err(err) => {
                    eprintln!("kleos-sh: exec failed: {}", err);
                    process::exit(1);
                }
            }
        }
    }
}
