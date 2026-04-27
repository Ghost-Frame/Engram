use std::env;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

// C-R3-003 / M-R3-006: replaced PATH-resolved `curl` shellout with an
// in-process reqwest::blocking client spawned on a detached thread. The
// previous implementation depended on whichever `curl` resolved first on
// $PATH, which on a hostile PATH is a CWE-426 hijack. The 1s timeout from
// the old curl --max-time 1 is preserved.

fn client() -> Option<&'static reqwest::blocking::Client> {
    static CLIENT: OnceLock<Option<reqwest::blocking::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_millis(500))
                .timeout(Duration::from_secs(1))
                .build()
                .ok()
        })
        .as_ref()
}

pub fn fire_and_forget(tool: &str, path: &str, gate_id: Option<&str>) {
    let sidecar_url =
        env::var("KLEOS_SIDECAR_URL").unwrap_or_else(|_| "http://127.0.0.1:4201".to_string());

    let agent = env::var("KLEOS_AGENT_SLOT").unwrap_or_else(|_| "unknown".to_string());

    let payload = serde_json::json!({
        "agent": agent,
        "tool_name": tool,
        "command": path,
        "gate_id": gate_id,
    });

    let url = format!("{}/observe", sidecar_url);

    // Best-effort, don't block on failure or on the network. The thread is
    // detached; if the parent exits first the OS reaps it.
    thread::Builder::new()
        .name("kleos-fs-observe".into())
        .spawn(move || {
            if let Some(c) = client() {
                let _ = c.post(&url).json(&payload).send();
            }
        })
        .ok();
}
