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
    // cred can be transiently unavailable (credd restart, YubiKey re-tap
    // window). Retry once with a 500ms backoff before giving up so a brief
    // outage does not push us straight into the fail-closed path.
    let slot = cred_slot();
    for attempt in 0..2 {
        let output = std::process::Command::new("cred")
            .args(["get", "kleos", &slot, "--raw"])
            .output()
            .ok()?;
        if output.status.success() {
            let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !key.is_empty() {
                return Some(key);
            }
        }
        if attempt == 0 {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    None
}

/// Fail-open opt-in. By default kleos-sh fails CLOSED when the gate is
/// unreachable or no API key is available; setting KLEOS_SH_FAIL_OPEN=1
/// reverts to the prior best-effort behaviour for local development.
fn fail_open_allowed() -> bool {
    matches!(
        std::env::var("KLEOS_SH_FAIL_OPEN").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Best-effort alert to Eidolon when the gate degrades. Fire-and-forget; we
/// never block the shell on the alert and we never propagate its errors.
fn alert_gate_degraded(
    client: &reqwest::Client,
    server_url: &str,
    api_key: Option<&str>,
    severity: &str,
    summary: &str,
) {
    let url = format!("{}/activity", server_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "agent": "kleos-sh",
        "action": if severity == "P0" { "error.raised" } else { "task.blocked" },
        "summary": summary,
        "project": "Kleos",
        "severity": severity,
    });
    let mut req = client.post(&url).json(&body);
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }
    tokio::spawn(async move {
        let _ = req.send().await;
    });
}

/// Slot identifier used to look up this agent's credential. Defaults to
/// `claude-code-{user}-{host}`. Override with `KLEOS_CRED_KEY` (preferred)
/// or `KLEOS_AGENT_SLOT` to pin to a specific slot.
fn cred_slot() -> String {
    if let Ok(slot) = std::env::var("KLEOS_CRED_KEY") {
        if !slot.is_empty() {
            return slot;
        }
    }
    if let Ok(slot) = std::env::var("KLEOS_AGENT_SLOT") {
        if !slot.is_empty() {
            return slot;
        }
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    format!("claude-code-{}-{}", user, read_hostname())
}

fn read_hostname() -> String {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let trimmed = h.trim().to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    "unknown-host".to_string()
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

    // Retry the gate up to four times with exponential backoff before deciding
    // the gate is genuinely unreachable. Most outages here are 1-2 second
    // restarts, not sustained. After the retries exhaust we fall closed
    // unless KLEOS_SH_FAIL_OPEN is explicitly set.
    let outcome = match &api_key {
        Some(key) => {
            let mut last_err: Option<String> = None;
            let mut delay_ms = 250u64;
            let mut got: Option<gate::GateOutcome> = None;
            for attempt in 0..4 {
                match gate::check_remote(&client, &server, key, &req).await {
                    Ok(outcome) => {
                        got = Some(outcome);
                        break;
                    }
                    Err(err) => {
                        last_err = Some(err);
                        if attempt < 3 {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            delay_ms *= 2;
                        }
                    }
                }
            }
            match got {
                Some(o) => o,
                None => {
                    let err_msg = last_err.unwrap_or_else(|| "unknown error".to_string());
                    if fail_open_allowed() {
                        eprintln!(
                            "kleos-sh: gate unreachable after retries ({}), failing OPEN per KLEOS_SH_FAIL_OPEN",
                            err_msg
                        );
                        alert_gate_degraded(
                            &client,
                            &server,
                            Some(key),
                            "P1",
                            &format!("kleos-sh gate unreachable, fail-open opt-in: {}", err_msg),
                        );
                        gate::GateOutcome::Allow {
                            command: command.clone(),
                            enrichment: None,
                            gate_id: 0,
                        }
                    } else {
                        eprintln!(
                            "kleos-sh: gate unreachable after retries ({}); failing CLOSED. Set KLEOS_SH_FAIL_OPEN=1 to override.",
                            err_msg
                        );
                        alert_gate_degraded(
                            &client,
                            &server,
                            Some(key),
                            "P0",
                            &format!("kleos-sh gate unreachable, fail-closed: {}", err_msg),
                        );
                        process::exit(2);
                    }
                }
            }
        }
        None => {
            if fail_open_allowed() {
                eprintln!(
                    "kleos-sh: no API key available, failing OPEN per KLEOS_SH_FAIL_OPEN"
                );
                alert_gate_degraded(
                    &client,
                    &server,
                    None,
                    "P1",
                    "kleos-sh missing API key, fail-open opt-in",
                );
                gate::GateOutcome::Allow {
                    command: command.clone(),
                    enrichment: None,
                    gate_id: 0,
                }
            } else {
                eprintln!(
                    "kleos-sh: no API key available; failing CLOSED. Set KLEOS_SH_FAIL_OPEN=1 to override (development only)."
                );
                alert_gate_degraded(
                    &client,
                    &server,
                    None,
                    "P0",
                    "kleos-sh missing API key, fail-closed",
                );
                process::exit(2);
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
