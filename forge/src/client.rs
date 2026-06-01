//! Kleos HTTP client with PIV/Ed25519/bearer auth.

use crate::error::{ForgeError, Result};

/// Authenticated HTTP client for the Kleos server.
pub struct KleosClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    signer: Option<kleos_lib::auth_piv::RequestSigner>,
}

// Method implementations for the authenticated Kleos HTTP client.
impl KleosClient {
    /// Bootstrap a client from environment and credential daemon.
    pub async fn from_env(base_url: &str) -> Result<Self> {
        let host_label = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".into());
        let agent_label = std::env::var("KLEOS_AGENT_LABEL").unwrap_or_else(|_| "forge".into());
        let model_label = std::env::var("KLEOS_MODEL_LABEL").unwrap_or_else(|_| "none".into());

        let signer = match kleos_lib::auth_piv::RequestSigner::from_env_or_file(
            &host_label,
            &agent_label,
            &model_label,
        ) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: identity key error: {e}");
                None
            }
        };

        let api_key = if let Some(k) = std::env::var("KLEOS_API_KEY")
            .ok()
            .or_else(|| kleos_lib::kleos_env("API_KEY").ok())
        {
            Some(k)
        } else {
            let slot = kleos_lib::cred::bootstrap::current_agent_slot();
            match kleos_lib::cred::bootstrap::resolve_api_key(&slot).await {
                Ok(k) => Some(k),
                Err(e) => {
                    if signer.is_none() {
                        eprintln!("warning: could not resolve API key: {e}");
                    }
                    None
                }
            }
        };

        if signer.is_none() && api_key.is_none() {
            return Err(ForgeError::Auth(
                "no signing identity or API key available".into(),
            ));
        }

        Ok(Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            signer,
        })
    }

    /// Send an authenticated GET request and return the response body as JSON.
    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.http.get(&url);
        let req = self.apply_auth(req, "GET", path, &[]);
        let resp = req.send().await.map_err(|e| {
            if e.is_connect() {
                ForgeError::Connection(e.to_string())
            } else {
                ForgeError::Transport(e)
            }
        })?;
        self.capture_session(&resp);
        self.handle_response(resp).await
    }

    /// Send an authenticated POST request with a JSON body.
    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let body_bytes = serde_json::to_vec(body)?;
        let req = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .body(body_bytes.clone());
        let req = self.apply_auth(req, "POST", path, &body_bytes);
        let resp = req.send().await.map_err(|e| {
            if e.is_connect() {
                ForgeError::Connection(e.to_string())
            } else {
                ForgeError::Transport(e)
            }
        })?;
        self.capture_session(&resp);
        self.handle_response(resp).await
    }

    /// Apply authentication headers to a request.
    fn apply_auth(
        &self,
        req: reqwest::RequestBuilder,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> reqwest::RequestBuilder {
        if let Some(signer) = &self.signer {
            if let Some(session) = signer.cached_session() {
                return req.header("X-Kleos-Session", session);
            }
            let (url_path, query) = match path.split_once('?') {
                Some((p, q)) => (p, q),
                None => (path, ""),
            };
            match signer.sign_request(method, url_path, query, body) {
                Ok(signed) => return signed.apply_headers(req),
                Err(e) => {
                    eprintln!("warning: PIV signing failed, falling back to API key: {e}");
                }
            }
        }
        if let Some(key) = &self.api_key {
            return req.bearer_auth(key);
        }
        req
    }

    /// Cache session tokens from response headers.
    fn capture_session(&self, resp: &reqwest::Response) {
        if let Some(signer) = &self.signer {
            if let Some(token) = resp.headers().get("x-kleos-session-issued") {
                if let Ok(t) = token.to_str() {
                    signer.set_session(t.to_string());
                }
            }
        }
    }

    /// Parse the response, returning JSON on success or a ForgeError on failure.
    async fn handle_response(&self, resp: reqwest::Response) -> Result<serde_json::Value> {
        let status = resp.status();
        if status.is_success() {
            let val: serde_json::Value = resp.json().await?;
            return Ok(val);
        }
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        if code == 401 || code == 403 {
            return Err(ForgeError::Auth(body));
        }
        Err(ForgeError::Server(code, body))
    }
}
