use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct GateCheckRequest {
    pub command: String,
    pub agent: String,
    pub context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GateCheckResult {
    pub allowed: bool,
    pub reason: Option<String>,
    pub resolved_command: Option<String>,
    pub gate_id: i64,
    #[allow(dead_code)]
    pub requires_approval: bool,
    pub enrichment: Option<String>,
}

pub enum GateOutcome {
    Allow {
        command: String,
        enrichment: Option<String>,
        gate_id: i64,
    },
    Deny {
        reason: String,
        #[allow(dead_code)]
        gate_id: i64,
    },
}

pub async fn check_remote(
    client: &reqwest::Client,
    server_url: &str,
    api_key: &str,
    req: &GateCheckRequest,
) -> Result<GateOutcome, String> {
    let url = format!("{}/gate/check", server_url.trim_end_matches('/'));

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(req)
        .send()
        .await
        .map_err(|e| format!("gate check request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() && status.as_u16() != 201 {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("gate check returned {}: {}", status, body));
    }

    let result: GateCheckResult = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse gate response: {}", e))?;

    if result.allowed {
        let command = result
            .resolved_command
            .unwrap_or_else(|| req.command.clone());
        Ok(GateOutcome::Allow {
            command,
            enrichment: result.enrichment,
            gate_id: result.gate_id,
        })
    } else {
        let reason = result
            .reason
            .unwrap_or_else(|| "denied by gate (no reason given)".to_string());
        Ok(GateOutcome::Deny {
            reason,
            gate_id: result.gate_id,
        })
    }
}
