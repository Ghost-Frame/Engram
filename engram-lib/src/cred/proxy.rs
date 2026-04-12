use std::collections::HashMap;

use reqwest::Method;
use serde::{Deserialize, Serialize};

use crate::cred::client::{CreddClient, FetchSecretRequest, SecretAccessMode};
use crate::db::Database;
use crate::webhooks::resolve_and_validate_url;
use crate::{EngError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_scheme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

impl CreddClient {
    pub async fn proxy(
        &self,
        db: &Database,
        user_id: i64,
        agent: &str,
        service: &str,
        key: &str,
        request: &ProxyRequest,
    ) -> Result<ProxyResponse> {
        // SECURITY (SSRF-DNS): validate the outbound URL against the SSRF
        // blocklist (loopback, RFC1918, link-local, cloud-metadata, IPv6 ULA)
        // AND resolve DNS to catch domains pointing at private IPs. Without
        // this the admin credential proxy forwards any URL including
        // 169.254.169.254/latest/meta-data.
        // Test clients set `allow_loopback_proxy` so mock HTTP servers on
        // 127.0.0.1 still work; production clients never do.
        if !self.allow_loopback_proxy {
            resolve_and_validate_url(&request.url).await.map_err(|e| match e {
                EngError::InvalidInput(msg) => {
                    EngError::InvalidInput(format!("cred proxy URL rejected: {}", msg))
                }
                other => other,
            })?;
        }

        let secret = self
            .fetch_secret_value(
                db,
                user_id,
                agent,
                FetchSecretRequest {
                    service,
                    key,
                    mode: SecretAccessMode::Resolved,
                    use_cache: false,
                },
            )
            .await?;

        let method = request
            .method
            .as_deref()
            .unwrap_or("GET")
            .parse::<Method>()
            .map_err(|e| EngError::InvalidInput(format!("invalid proxy method: {}", e)))?;

        let header_name = request
            .auth_header
            .clone()
            .unwrap_or_else(|| "Authorization".to_string());
        let header_value = match request.auth_scheme.as_deref() {
            Some("") => secret,
            Some(scheme) => format!("{} {}", scheme.trim(), secret),
            None => format!("Bearer {}", secret),
        };

        let mut builder = self.request(method, &request.url);
        if let Some(headers) = &request.headers {
            for (name, value) in headers {
                builder = builder.header(name, value);
            }
        }
        builder = builder.header(&header_name, header_value);
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }

        let response = builder
            .send()
            .await
            .map_err(|e| EngError::Internal(format!("proxy request failed: {}", e)))?;
        let status = response.status().as_u16();

        let mut headers = HashMap::new();
        for (name, value) in response.headers().iter() {
            if let Ok(text) = value.to_str() {
                headers.insert(name.to_string(), text.to_string());
            }
        }

        let body = response
            .text()
            .await
            .map_err(|e| EngError::Internal(format!("proxy response read failed: {}", e)))?;

        Ok(ProxyResponse {
            status,
            headers,
            body,
        })
    }
}
