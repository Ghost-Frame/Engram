use serde::Serialize;

#[derive(Serialize)]
struct ObservePayload {
    agent: String,
    tool_name: String,
    command: String,
    gate_id: i64,
    exit_code: i32,
}

pub fn fire_and_forget(
    client: &reqwest::Client,
    sidecar_url: &str,
    api_key: &str,
    agent: &str,
    command: &str,
    gate_id: i64,
    exit_code: i32,
) {
    let payload = ObservePayload {
        agent: agent.to_string(),
        tool_name: "Bash".to_string(),
        command: command.to_string(),
        gate_id,
        exit_code,
    };

    let url = format!("{}/observe", sidecar_url.trim_end_matches('/'));
    let client = client.clone();
    let api_key = api_key.to_string();

    tokio::spawn(async move {
        let _ = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&payload)
            .send()
            .await;
    });
}
