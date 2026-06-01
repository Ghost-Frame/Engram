//! `Client` is the canonical HTTP client used to talk to `kleos-server`.
//!
//! Lifted verbatim from `kleos-cli/src/main.rs` so both `kleos-cli` and
//! `kleos-mcp` (and any future Rust consumer) share one signing path.

use crate::routes::{render_path, Method, Route};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use std::time::Duration;

/// HTTP client wrapper that handles auth, session capture, and base-URL composition.
///
/// Supports comma-separated URLs in `KLEOS_URL` for failover: the first URL is
/// the primary, subsequent URLs are tried on connection-level failures (timeout,
/// refused, unreachable). HTTP-level errors (4xx, 5xx) are NOT retried.
pub struct Client {
    http: reqwest::Client,
    urls: Vec<String>,
    api_key: Option<String>,
    pub signer: Option<kleos_lib::auth_piv::RequestSigner>,
}

/// Constructor and HTTP request helpers for `Client`.
impl Client {
    /// Constructs a new `Client`. `base_url` may be comma-separated for failover
    /// (e.g. `"http://primary-host:4200,http://backup-host:4200"`).
    pub fn new(
        base_url: String,
        api_key: Option<String>,
        signer: Option<kleos_lib::auth_piv::RequestSigner>,
    ) -> Self {
        let urls: Vec<String> = base_url
            .split(',')
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Self {
            http: reqwest::Client::new(),
            urls,
            api_key,
            signer,
        }
    }

    /// Returns the primary (first) base URL.
    pub fn base_url(&self) -> &str {
        self.urls.first().map(|s| s.as_str()).unwrap_or("")
    }

    /// Applies PIV-signed headers or bearer-token auth to a pending request.
    pub fn apply_auth(
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

    /// Core request dispatcher with URL failover. Tries each configured URL in
    /// order; on connection-level failures (timeout, refused, unreachable) falls
    /// through to the next URL. HTTP errors (4xx, 5xx) are returned immediately.
    async fn execute(
        &self,
        http: &reqwest::Client,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        content_type: Option<&str>,
    ) -> Result<reqwest::Response, String> {
        let mut last_err = String::new();
        for (i, base) in self.urls.iter().enumerate() {
            let url = format!("{base}{path}");
            let mut req = match method {
                "GET" => http.get(&url),
                "POST" => http.post(&url),
                "PUT" => http.put(&url),
                "PATCH" => http.patch(&url),
                "DELETE" => http.delete(&url),
                _ => return Err(format!("unsupported HTTP method: {method}")),
            };
            if let Some(ct) = content_type {
                req = req.header("content-type", ct);
            }
            if let Some(b) = body {
                req = req.body(b.to_vec());
            }
            req = self.apply_auth(req, method, path, body.unwrap_or(b""));
            match req.send().await {
                Ok(resp) => return Ok(resp),
                Err(e) if is_connection_error(&e) && i + 1 < self.urls.len() => {
                    eprintln!(
                        "warning: {method} {url} failed ({}), trying next URL",
                        format_error_chain(&e)
                    );
                    last_err = format_reqwest_error(method, &url, &e);
                }
                Err(e) => return Err(format_reqwest_error(method, &url, &e)),
            }
        }
        Err(last_err)
    }

    /// Reads any session token issued by the server and caches it in the signer.
    pub fn capture_session(&self, resp: &reqwest::Response) {
        if let Some(signer) = &self.signer {
            if let Some(token) = resp.headers().get("x-kleos-session-issued") {
                if let Ok(t) = token.to_str() {
                    signer.set_session(t.to_string());
                }
            }
        }
    }

    /// Sends an authenticated GET request and returns the parsed JSON body.
    pub async fn get(&self, path: &str) -> Result<Value, String> {
        let resp = self.execute(&self.http, "GET", path, None, None).await?;
        self.capture_session(&resp);
        self.handle_response("GET", path, resp).await
    }

    /// Sends an authenticated POST request with a JSON body and returns the parsed JSON response.
    pub async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
        let resp = self
            .execute(
                &self.http,
                "POST",
                path,
                Some(&body_bytes),
                Some("application/json"),
            )
            .await?;
        self.capture_session(&resp);
        self.handle_response("POST", path, resp).await
    }

    /// Sends an authenticated PUT request with a JSON body and returns the parsed JSON response.
    pub async fn put(&self, path: &str, body: Value) -> Result<Value, String> {
        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
        let resp = self
            .execute(
                &self.http,
                "PUT",
                path,
                Some(&body_bytes),
                Some("application/json"),
            )
            .await?;
        self.capture_session(&resp);
        self.handle_response("PUT", path, resp).await
    }

    /// Sends an authenticated PATCH request with a JSON body and returns the parsed JSON response.
    pub async fn patch(&self, path: &str, body: Value) -> Result<Value, String> {
        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
        let resp = self
            .execute(
                &self.http,
                "PATCH",
                path,
                Some(&body_bytes),
                Some("application/json"),
            )
            .await?;
        self.capture_session(&resp);
        self.handle_response("PATCH", path, resp).await
    }

    /// Sends an authenticated DELETE request and returns the parsed JSON response.
    pub async fn delete(&self, path: &str) -> Result<Value, String> {
        let resp = self.execute(&self.http, "DELETE", path, None, None).await?;
        self.capture_session(&resp);
        self.handle_response("DELETE", path, resp).await
    }

    /// Sends an authenticated multipart POST request and returns the parsed JSON response.
    /// No failover -- multipart forms cannot be resent.
    pub async fn post_multipart(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url(), path);
        let req = self.http.post(&url).multipart(form);
        let req = self.apply_auth(req, "POST", path, b"");
        let resp = req
            .send()
            .await
            .map_err(|e| format_reqwest_error("POST", &url, &e))?;
        self.capture_session(&resp);
        self.handle_response("POST", path, resp).await
    }

    /// Sends an authenticated GET and returns the raw bytes, filename, and content-type.
    pub async fn get_bytes(&self, path: &str) -> Result<(Vec<u8>, String, String), String> {
        let resp = self.execute(&self.http, "GET", path, None, None).await?;
        self.capture_session(&resp);
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("HTTP {}: {}", status, text));
        }
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();
        let filename = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.split("filename=")
                    .nth(1)
                    .map(|f| f.trim_matches('"').to_string())
            })
            .unwrap_or_else(|| "artifact".to_string());
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        Ok((bytes.to_vec(), filename, content_type))
    }

    /// Interprets an HTTP response, returning parsed JSON on success or an error string on failure.
    pub async fn handle_response(
        &self,
        method: &str,
        path: &str,
        resp: reqwest::Response,
    ) -> Result<Value, String> {
        let status = resp.status();
        let url = resp.url().to_string();

        if status == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(signer) = &self.signer {
                signer.clear_session();
            }
        }

        let bytes = resp.bytes().await.map_err(|e| {
            format!(
                "{} {} succeeded but reading response body failed: {}",
                method,
                path,
                format_error_chain(&e)
            )
        })?;
        let parsed: Result<Value, _> = serde_json::from_slice(&bytes);
        if status.is_success() {
            parsed.or_else(|_| {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                Ok(serde_json::json!({ "content": text }))
            })
        } else {
            let msg = parsed
                .as_ref()
                .ok()
                .and_then(|b| {
                    b.get("error")
                        .or_else(|| b.get("message"))
                        .and_then(|v| v.as_str())
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(|| body_excerpt(&bytes));
            Err(format!("HTTP {} {}: {}", status, url, msg))
        }
    }

    /// Sends a GET request with a per-call timeout; returns an empty object on 404.
    pub async fn get_with_timeout(&self, path: &str, timeout: Duration) -> Result<Value, String> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| format!("http client build failed: {e}"))?;
        let resp = self.execute(&http, "GET", path, None, None).await?;
        self.capture_session(&resp);
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() || status.as_u16() == 404 {
            Ok(serde_json::from_str(&text).unwrap_or(json!({})))
        } else {
            Err(format!("HTTP {}: {}", status, text))
        }
    }

    /// Sends a POST request with a per-call timeout; useful for fire-and-forget activity reports.
    pub async fn post_with_timeout(
        &self,
        path: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| format!("http client build failed: {e}"))?;
        let resp = self
            .execute(
                &http,
                "POST",
                path,
                Some(&body_bytes),
                Some("application/json"),
            )
            .await?;
        self.capture_session(&resp);
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if status.is_success() {
            Ok(serde_json::from_str(&text).unwrap_or(json!({"ok": true})))
        } else {
            Err(format!("HTTP {}: {}", status, text))
        }
    }

    /// Dispatches a registered `Route`, filling path parameters from `args`,
    /// applying the route's method, and returning the JSON response.
    ///
    /// `args` is mutated in place: keys consumed by the path template are
    /// removed before the body is serialised so they do not double-up.
    /// For GET/DELETE, remaining args become query-string parameters.
    pub async fn call_route(&self, route: &Route, mut args: Value) -> Result<Value, String> {
        let path = render_path(route.path, &mut args)?;
        match route.method {
            Method::Get => {
                let full = append_query_string(&path, &args);
                self.get(&full).await
            }
            Method::Delete => {
                let full = append_query_string(&path, &args);
                self.delete(&full).await
            }
            Method::Post => self.post(&path, args).await,
            Method::Put => self.put(&path, args).await,
            Method::Patch => self.patch(&path, args).await,
        }
    }

    /// Forwards a raw JSON-RPC envelope to the server-side POST /mcp
    /// endpoint. On 401, clears any stale cached session and retries once
    /// so the request falls through to PIV signing or bearer auth.
    pub async fn post_mcp(&self, body: &Value) -> Result<Option<Value>, String> {
        let (status, val) = self.post_mcp_once(body).await?;

        if status == reqwest::StatusCode::UNAUTHORIZED {
            if let Some(signer) = &self.signer {
                signer.clear_session();
            }
            let (retry_status, retry_val) = self.post_mcp_once(body).await?;
            if retry_status.is_success()
                || retry_status.as_u16() == 202
                || retry_status.as_u16() == 204
            {
                return Ok(retry_val);
            }
            if let Some(v) = retry_val {
                if let Some(err) = v.get("_mcp_error").and_then(|e| e.as_str()) {
                    return Err(err.to_string());
                }
            }
            return Err(format!(
                "POST /mcp (HTTP {retry_status}): auth failed after retry"
            ));
        }

        if status.as_u16() == 202 || status.as_u16() == 204 {
            return Ok(None);
        }
        if status.is_success() {
            return Ok(val);
        }
        if let Some(v) = val {
            if let Some(err) = v.get("_mcp_error").and_then(|e| e.as_str()) {
                return Err(err.to_string());
            }
        }
        Err(format!("POST /mcp (HTTP {status}): unexpected error"))
    }

    /// Sends a single POST /mcp request and interprets the response.
    async fn post_mcp_once(
        &self,
        body: &Value,
    ) -> Result<(reqwest::StatusCode, Option<Value>), String> {
        let body_bytes = serde_json::to_vec(body).unwrap_or_default();
        let resp = self
            .execute(
                &self.http,
                "POST",
                "/mcp",
                Some(&body_bytes),
                Some("application/json"),
            )
            .await?;
        self.capture_session(&resp);
        let status = resp.status();

        if status.as_u16() == 202 || status.as_u16() == 204 {
            return Ok((status, None));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("POST /mcp: reading response body failed: {e}"))?;
        if status.is_success() {
            let parsed: Value = serde_json::from_slice(&bytes)
                .map_err(|e| format!("POST /mcp: invalid JSON: {e}"))?;
            Ok((status, Some(parsed)))
        } else {
            let msg = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|b| {
                    b.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(|| body_excerpt(&bytes));
            Ok((
                status,
                Some(
                    serde_json::json!({"_mcp_error": format!("POST /mcp (HTTP {status}): {msg}")}),
                ),
            ))
        }
    }

    /// Returns the agent label from the PIV signer, or "claude-code" when no signer is configured.
    pub fn agent_label(&self) -> String {
        self.signer
            .as_ref()
            .map(|s| s.agent_label().to_string())
            .unwrap_or_else(|| "claude-code".to_string())
    }
}

/// Formats a reqwest transport error with method and URL context.
pub fn format_reqwest_error(method: &str, url: &str, err: &reqwest::Error) -> String {
    format!("{} {} failed: {}", method, url, format_error_chain(err))
}

/// Walks the error source chain and concatenates messages with " -> " separators.
pub fn format_error_chain<E: std::error::Error + ?Sized>(err: &E) -> String {
    let mut out = err.to_string();
    let mut source: Option<&dyn std::error::Error> = err.source();
    for _ in 0..16 {
        let Some(cause) = source else { break };
        out.push_str(" -> ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

/// Appends remaining JSON object fields as query-string parameters.
/// Scalar values become `key=value`; arrays become repeated `key=v1&key=v2`;
/// null values and empty objects are skipped.
fn append_query_string(path: &str, args: &Value) -> String {
    let map = match args.as_object() {
        Some(m) if !m.is_empty() => m,
        _ => return path.to_string(),
    };
    let mut qs = String::new();
    for (k, v) in map {
        match v {
            Value::Null => continue,
            Value::Object(inner) if inner.is_empty() => continue,
            Value::Array(arr) => {
                for item in arr {
                    push_qparam(&mut qs, k, item);
                }
            }
            _ => push_qparam(&mut qs, k, v),
        }
    }
    if qs.is_empty() {
        return path.to_string();
    }
    format!("{path}?{}", &qs[1..]) // skip leading '&'
}

fn push_qparam(qs: &mut String, key: &str, val: &Value) {
    let encoded_key = utf8_percent_encode(key, NON_ALPHANUMERIC);
    let raw = match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return,
    };
    let encoded_val = utf8_percent_encode(&raw, NON_ALPHANUMERIC);
    qs.push('&');
    qs.push_str(&encoded_key.to_string());
    qs.push('=');
    qs.push_str(&encoded_val.to_string());
}

/// Returns true for connection-level failures that warrant URL failover.
fn is_connection_error(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout()
}

/// Returns up to 512 bytes of the response body as a UTF-8 string for error messages.
pub fn body_excerpt(bytes: &[u8]) -> String {
    const MAX: usize = 512;
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= MAX {
        return s.into_owned();
    }
    let mut end = MAX;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... ({} bytes total)", &s[..end], bytes.len())
}

/// Truncates a string to the given byte length.
pub fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..max]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_string_from_scalars() {
        let args = json!({"limit": 10, "offset": 5});
        let result = append_query_string("/list", &args);
        assert!(result.starts_with("/list?"));
        assert!(result.contains("limit=10"));
        assert!(result.contains("offset=5"));
    }

    #[test]
    fn query_string_skips_null_and_empty_object() {
        let args = json!({"limit": 10, "filter": null, "opts": {}});
        let result = append_query_string("/list", &args);
        assert!(result.contains("limit=10"));
        assert!(!result.contains("filter"));
        assert!(!result.contains("opts"));
    }

    #[test]
    fn query_string_empty_args() {
        assert_eq!(append_query_string("/list", &json!({})), "/list");
        assert_eq!(append_query_string("/list", &json!(null)), "/list");
    }

    #[test]
    fn query_string_encodes_special_chars() {
        let args = json!({"q": "hello world&more"});
        let result = append_query_string("/search", &args);
        assert!(result.starts_with("/search?"));
        assert!(!result.contains(' '));
        assert!(result.contains("q="));
    }

    #[test]
    fn query_string_array_repeats_key() {
        let args = json!({"tags": ["bug", "fix"]});
        let result = append_query_string("/list", &args);
        let count = result.matches("tags=").count();
        assert_eq!(count, 2);
    }
}
