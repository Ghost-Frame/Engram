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

    /// Send an authenticated GET request with JSON object params serialized as a query string.
    ///
    /// `body` must be a `serde_json::Value::Object`. Each key-value pair is appended to
    /// the URL as `?k=v&...`; non-string values are coerced via their JSON representation.
    /// This is the correct path for GET-method skill dispatch (see exec.rs).
    pub async fn get_with_query(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Flatten the JSON object into (String, String) pairs for URL encoding.
        // Values that are not plain strings are serialized as their JSON text so that
        // integers and booleans round-trip correctly through the query string.
        let pairs: Vec<(String, String)> = body
            .as_object()
            .map(|obj| {
                obj.iter()
                    .map(|(k, v)| {
                        let s = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (k.clone(), s)
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Build the query string so we can pass it to apply_auth for signing.
        let qs: String = pairs
            .iter()
            .enumerate()
            .map(|(i, (k, v))| {
                let sep = if i == 0 { "" } else { "&" };
                format!(
                    "{}{}={}",
                    sep,
                    percent_encode_query(k),
                    percent_encode_query(v)
                )
            })
            .collect();

        // Construct the full URL; apply_auth will split on '?' for signing.
        let full_path = if qs.is_empty() {
            path.to_string()
        } else {
            format!("{}?{}", path, qs)
        };
        let url = format!("{}{}", self.base_url, full_path);
        let req = self.http.get(&url);
        let req = self.apply_auth(req, "GET", &full_path, &[]);
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

/// Percent-encode a single query-string component (key or value).
///
/// Encodes all bytes except unreserved characters (A-Z a-z 0-9 - _ . ~)
/// per RFC 3986. Spaces become `%20`, not `+`.
fn percent_encode_query(s: &str) -> String {
    // Characters that do NOT need encoding in a query component.
    const UNRESERVED: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        if UNRESERVED.contains(&byte) {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(
                char::from_digit((byte >> 4) as u32, 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
            out.push(
                char::from_digit((byte & 0xF) as u32, 16)
                    .unwrap_or('0')
                    .to_ascii_uppercase(),
            );
        }
    }
    out
}
