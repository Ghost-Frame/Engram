//! Blocking HTTP client for the Kleos API. Covers the skills sub-API used by
//! agent-forge tools to search, capture, record execution of, fix, derive,
//! and query lineage of skills stored in the Kleos service.

use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::env;
use std::time::Duration;

/// Errors that can arise when using `KleosClient`. Each variant carries a
/// human-readable message that describes the specific failure.
#[derive(Debug)]
#[allow(dead_code)]
pub enum KleosClientError {
    /// The client could not be configured -- typically a missing env var.
    NotConfigured(String),
    /// The HTTP request was sent but the server returned an error status.
    RequestFailed(String),
    /// The server replied with a non-JSON or structurally unexpected body.
    InvalidResponse(String),
}

/// Render `KleosClientError` as a short human-readable string.
impl std::fmt::Display for KleosClientError {
    /// Format the error as a concise message suitable for CLI output or logging.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KleosClientError::NotConfigured(s) => write!(f, "Kleos not configured: {}", s),
            KleosClientError::RequestFailed(s) => write!(f, "Kleos request failed: {}", s),
            KleosClientError::InvalidResponse(s) => write!(f, "Kleos invalid response: {}", s),
        }
    }
}

/// Marker impl so `KleosClientError` integrates with `?` and `dyn Error` chains.
impl std::error::Error for KleosClientError {}

/// Blocking HTTP client for the Kleos REST API. Holds the base URL, an
/// optional Bearer token, and a shared `reqwest` connection pool.
pub struct KleosClient {
    http: Client,
    base_url: String,
    api_key: Option<String>,
}

/// Methods for constructing the client and calling the Kleos skills API.
impl KleosClient {
    /// Create a new client. Returns Err if KLEOS_URL is not set and default is unreachable.
    pub fn new() -> Result<Self, KleosClientError> {
        let base_url =
            env::var("KLEOS_URL").unwrap_or_else(|_| "http://localhost:4200".to_string());
        let base_url = base_url.trim_end_matches('/').to_string();
        // Keyless fallback: short-lived bearer from the phylaxd SO_PEERCRED broker.
        let api_key = env::var("KLEOS_API_KEY")
            .ok()
            .filter(|k| !k.is_empty())
            .or_else(kleos_token_client::resolve_via_phylax_broker);

        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| KleosClientError::RequestFailed(e.to_string()))?;

        Ok(Self {
            http,
            base_url,
            api_key,
        })
    }

    /// Attach a `Bearer` token to the request if an API key is configured.
    fn apply_auth(
        &self,
        req: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        if let Some(key) = &self.api_key {
            req.bearer_auth(key)
        } else {
            req
        }
    }

    /// Execute a GET request against `path` (relative to `base_url`) and
    /// deserialize the JSON response body.
    fn get(&self, path: &str) -> Result<Value, KleosClientError> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.http.get(&url);
        let req = self.apply_auth(req);
        let resp = req
            .send()
            .map_err(|e| KleosClientError::RequestFailed(format!("{}: {}", url, e)))?;
        if !resp.status().is_success() {
            return Err(KleosClientError::RequestFailed(format!(
                "{}: HTTP {}",
                url,
                resp.status()
            )));
        }
        resp.json::<Value>()
            .map_err(|e| KleosClientError::InvalidResponse(e.to_string()))
    }

    /// Execute a POST request with a JSON body against `path` (relative to
    /// `base_url`) and deserialize the JSON response body.
    fn post(&self, path: &str, body: Value) -> Result<Value, KleosClientError> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.http.post(&url).json(&body);
        let req = self.apply_auth(req);
        let resp = req
            .send()
            .map_err(|e| KleosClientError::RequestFailed(format!("{}: {}", url, e)))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().unwrap_or_default();
            return Err(KleosClientError::RequestFailed(format!(
                "{}: HTTP {} -- {}",
                url, status, body_text
            )));
        }
        resp.json::<Value>()
            .map_err(|e| KleosClientError::InvalidResponse(e.to_string()))
    }

    // --- Skill API methods ---

    /// Search for skills matching `query`, optionally capped at `limit` results.
    pub fn search_skills(
        &self,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Value, KleosClientError> {
        let mut body = json!({ "query": query });
        if let Some(l) = limit {
            body["limit"] = json!(l);
        }
        self.post("/skills/search", body)
    }

    /// Fetch a single skill by its numeric ID.
    #[allow(dead_code)]
    pub fn get_skill(&self, id: i64) -> Result<Value, KleosClientError> {
        self.get(&format!("/skills/{}", id))
    }

    /// Submit a new skill to Kleos with the given natural-language description,
    /// optionally tagging it with the originating `agent` identifier.
    pub fn capture_skill(
        &self,
        description: &str,
        agent: Option<&str>,
    ) -> Result<Value, KleosClientError> {
        let mut body = json!({ "description": description });
        if let Some(a) = agent {
            body["agent"] = json!(a);
        }
        self.post("/skills/capture", body)
    }

    /// Record one execution attempt for `skill_id`, noting whether it succeeded,
    /// how long it took, and any error details if it failed.
    pub fn record_execution(
        &self,
        skill_id: i64,
        success: bool,
        duration_ms: Option<f64>,
        error_type: Option<&str>,
        error_message: Option<&str>,
    ) -> Result<Value, KleosClientError> {
        let mut body = json!({ "success": success });
        if let Some(d) = duration_ms {
            body["duration_ms"] = json!(d);
        }
        if let Some(et) = error_type {
            body["error_type"] = json!(et);
        }
        if let Some(em) = error_message {
            body["error_message"] = json!(em);
        }
        self.post(&format!("/skills/{}/execute", skill_id), body)
    }

    /// Request Kleos to create a corrected version of `skill_id`, optionally
    /// guiding the fix with a free-text `hint`.
    pub fn fix_skill(&self, skill_id: i64, hint: Option<&str>) -> Result<Value, KleosClientError> {
        let mut body = json!({});
        if let Some(h) = hint {
            body["hint"] = json!(h);
        }
        self.post(&format!("/skills/{}/fix", skill_id), body)
    }

    /// Derive a new skill from one or more parent skills. `direction` is a
    /// natural-language prompt describing how to mutate or combine the parents.
    pub fn derive_skill(
        &self,
        parent_ids: &[i64],
        direction: &str,
        agent: Option<&str>,
    ) -> Result<Value, KleosClientError> {
        let mut body = json!({
            "parent_ids": parent_ids,
            "direction": direction,
        });
        if let Some(a) = agent {
            body["agent"] = json!(a);
        }
        self.post("/skills/derive", body)
    }

    /// Retrieve the full derivation lineage (ancestor and descendant chain) for
    /// the given `skill_id`.
    pub fn get_lineage(&self, skill_id: i64) -> Result<Value, KleosClientError> {
        self.get(&format!("/skills/{}/lineage", skill_id))
    }

    /// List skills with pagination support, optionally filtered to a single `agent`.
    #[allow(dead_code)]
    pub fn list_skills(
        &self,
        limit: usize,
        offset: usize,
        agent: Option<&str>,
    ) -> Result<Value, KleosClientError> {
        let mut path = format!("/skills?limit={}&offset={}", limit, offset);
        if let Some(a) = agent {
            path.push_str(&format!("&agent={}", a));
        }
        self.get(&path)
    }
}
