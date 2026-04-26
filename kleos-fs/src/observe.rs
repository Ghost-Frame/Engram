use std::env;
use std::process::Command;

pub fn fire_and_forget(tool: &str, path: &str, gate_id: Option<&str>) {
    let sidecar_url = env::var("KLEOS_SIDECAR_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:4201".to_string());

    let agent = env::var("KLEOS_AGENT_SLOT")
        .unwrap_or_else(|_| "unknown".to_string());

    let payload = serde_json::json!({
        "agent": agent,
        "tool_name": tool,
        "command": path,
        "gate_id": gate_id,
    });

    let url = format!("{}/observe", sidecar_url);

    // Best-effort, don't block on failure
    let _ = Command::new("curl")
        .arg("-sf")
        .arg("--max-time")
        .arg("1")
        .arg(&url)
        .arg("-X")
        .arg("POST")
        .arg("-H")
        .arg("Content-Type: application/json")
        .arg("-d")
        .arg(payload.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}
