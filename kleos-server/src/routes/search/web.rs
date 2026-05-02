use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

/// Allowed SearXNG category values. Anything else is rejected so operators
/// cannot pass arbitrary upstream parameters through the proxy.
const ALLOWED_CATEGORIES: &[&str] = &[
    "general",
    "images",
    "videos",
    "news",
    "map",
    "music",
    "it",
    "science",
    "files",
    "social+media",
    "social media",
];

const HARD_LIMIT_MAX: u32 = 50;

// M-R3-005: previously a `LazyLock<Client>` with `.expect("...")` at the
// build call. If the builder ever returned Err (TLS misconfig, native-tls
// vs rustls feature drift, ...), the panic would poison the LazyLock slot
// and every subsequent /search/web request would also panic on touch.
// LazyLock cannot recover, so the route became permanently dead until a
// process restart.
//
// Switching to `OnceLock<Result<Client, String>>` means a builder failure
// caches the error string and the handler returns 503 SERVICE_UNAVAILABLE
// instead of panicking. The Client itself is still built once for the
// lifetime of the process so the connection pool keeps working.
static SEARXNG_CLIENT: std::sync::OnceLock<std::result::Result<reqwest::Client, String>> =
    std::sync::OnceLock::new();

fn searxng_client() -> std::result::Result<&'static reqwest::Client, &'static str> {
    SEARXNG_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(3))
                .timeout(std::time::Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("Kleos/1.0 (search-proxy)")
                .pool_max_idle_per_host(4)
                .build()
                .map_err(|e| format!("reqwest::Client::builder failed: {}", e))
        })
        .as_ref()
        .map_err(|s| s.as_str())
}

#[derive(Debug, Deserialize)]
pub(super) struct WebSearchBody {
    pub query: String,
    #[serde(default)]
    pub categories: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub pageno: Option<u32>,
    #[serde(default)]
    pub safesearch: Option<u8>,
    #[serde(default)]
    pub limit: Option<u32>,
}

// H-R3-002: web search proxies SearXNG; no DB access, no tenant data
// touched. Auth(_auth) is intentional.
pub(super) async fn web_search(
    State(state): State<AppState>,
    Auth(_auth): Auth,
    Json(body): Json<WebSearchBody>,
) -> Result<axum::response::Response, AppError> {
    let query = body.query.trim();
    if query.is_empty() {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "query must not be empty".into(),
        )));
    }
    if query.len() > 512 {
        return Err(AppError::from(kleos_lib::EngError::InvalidInput(
            "query too long (max 512 chars)".into(),
        )));
    }

    let categories = match body.categories.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(c) if ALLOWED_CATEGORIES.contains(&c) => Some(c.to_string()),
        Some(c) => {
            return Err(AppError::from(kleos_lib::EngError::InvalidInput(format!(
                "unknown category '{}'",
                c
            ))));
        }
    };

    let language = match body.language.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(l) if l.len() <= 10 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
            Some(l.to_string())
        }
        Some(_) => {
            return Err(AppError::from(kleos_lib::EngError::InvalidInput(
                "invalid language code".into(),
            )));
        }
    };

    let pageno = body.pageno.unwrap_or(1).clamp(1, 20);
    let safesearch = body.safesearch.map(|s| s.min(2));
    let limit = body
        .limit
        .unwrap_or(state.config.web_search_default_limit)
        .clamp(1, HARD_LIMIT_MAX);

    let base = state.config.web_search_url.trim_end_matches('/');
    let url = format!("{}/search", base);

    let mut form: Vec<(&str, String)> = vec![
        ("q", query.to_string()),
        ("format", "json".to_string()),
        ("pageno", pageno.to_string()),
    ];
    if let Some(c) = &categories {
        form.push(("categories", c.clone()));
    }
    if let Some(l) = &language {
        form.push(("language", l.clone()));
    }
    if let Some(s) = safesearch {
        form.push(("safesearch", s.to_string()));
    }

    let client = searxng_client().map_err(|e| {
        tracing::error!(error = %e, "searxng client unavailable");
        AppError::from(kleos_lib::EngError::Internal(e.to_string()))
    })?;

    let req_fut = client
        .get(&url)
        .timeout(std::time::Duration::from_millis(
            state.config.web_search_timeout_ms,
        ))
        .query(&form)
        .send();

    let resp = match req_fut.await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "searxng request failed");
            return Ok(upstream_error("search upstream unreachable"));
        }
    };

    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "searxng non-2xx");
        return Ok(upstream_error("search upstream returned error"));
    }

    let body_val: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "searxng non-json response");
            return Ok(upstream_error("search upstream returned invalid JSON"));
        }
    };

    let normalized = normalize_searxng(&body_val, limit as usize);
    Ok((StatusCode::OK, Json(normalized)).into_response())
}

fn upstream_error(msg: &str) -> axum::response::Response {
    (StatusCode::BAD_GATEWAY, Json(json!({ "error": msg }))).into_response()
}

/// Trim the SearXNG JSON response down to a compact, stable schema.
/// Only the fields agents need: title, url, snippet, engine, score, plus
/// optional image/thumbnail for image results.
pub(super) fn normalize_searxng(raw: &Value, limit: usize) -> Value {
    let query = raw.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let number_of_results = raw
        .get("number_of_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let results: Vec<Value> = raw
        .get("results")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .take(limit)
                .map(|r| {
                    let title = r.get("title").and_then(|v| v.as_str()).unwrap_or("");
                    let url = r.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let snippet = r
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(500)
                        .collect::<String>();
                    let engine = r.get("engine").and_then(|v| v.as_str()).unwrap_or("");
                    let score = r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let category = r.get("category").and_then(|v| v.as_str()).unwrap_or("");
                    let mut out = json!({
                        "title": title,
                        "url": url,
                        "snippet": snippet,
                        "engine": engine,
                        "category": category,
                        "score": score,
                    });
                    if let Some(img) = r.get("img_src").and_then(|v| v.as_str()) {
                        out["img_src"] = json!(img);
                    }
                    if let Some(thumb) = r.get("thumbnail_src").and_then(|v| v.as_str()) {
                        out["thumbnail_src"] = json!(thumb);
                    }
                    if let Some(pub_date) = r.get("publishedDate").and_then(|v| v.as_str()) {
                        out["published_at"] = json!(pub_date);
                    }
                    out
                })
                .filter(|r| {
                    !r.get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .is_empty()
                })
                .collect()
        })
        .unwrap_or_default();

    let suggestions: Vec<Value> = raw
        .get("suggestions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let count = results.len();
    json!({
        "query": query,
        "number_of_results": number_of_results,
        "results": results,
        "suggestions": suggestions,
        "count": count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_core_fields() {
        let raw = json!({
            "query": "rust async",
            "number_of_results": 42,
            "results": [
                {
                    "title": "Async Rust",
                    "url": "https://example.com/1",
                    "content": "An intro to async Rust.",
                    "engine": "google",
                    "category": "general",
                    "score": 1.5
                }
            ],
            "suggestions": ["rust tokio"]
        });
        let norm = normalize_searxng(&raw, 10);
        assert_eq!(norm["query"], "rust async");
        assert_eq!(norm["count"], 1);
        let first = &norm["results"][0];
        assert_eq!(first["title"], "Async Rust");
        assert_eq!(first["url"], "https://example.com/1");
        assert_eq!(first["snippet"], "An intro to async Rust.");
        assert_eq!(first["engine"], "google");
        assert_eq!(first["score"], 1.5);
    }

    #[test]
    fn normalize_drops_results_without_url() {
        let raw = json!({
            "results": [
                { "title": "no url", "content": "oops" },
                { "title": "good", "url": "https://example.com", "content": "hi" }
            ]
        });
        let norm = normalize_searxng(&raw, 10);
        assert_eq!(norm["count"], 1);
        assert_eq!(norm["results"][0]["url"], "https://example.com");
    }

    #[test]
    fn normalize_respects_limit() {
        let results: Vec<Value> = (0..20)
            .map(|i| {
                json!({
                    "title": format!("t{}", i),
                    "url": format!("https://example.com/{}", i),
                    "content": "",
                    "engine": "e",
                    "score": 0.0
                })
            })
            .collect();
        let raw = json!({ "results": results });
        let norm = normalize_searxng(&raw, 5);
        assert_eq!(norm["count"], 5);
    }

    #[test]
    fn normalize_handles_missing_fields() {
        let raw = json!({ "results": [{ "url": "https://x.test" }] });
        let norm = normalize_searxng(&raw, 10);
        assert_eq!(norm["count"], 1);
        let r = &norm["results"][0];
        assert_eq!(r["title"], "");
        assert_eq!(r["snippet"], "");
        assert_eq!(r["engine"], "");
    }

    #[test]
    fn normalize_truncates_long_snippet() {
        let long = "x".repeat(2000);
        let raw = json!({
            "results": [{
                "title": "t",
                "url": "https://x.test",
                "content": long
            }]
        });
        let norm = normalize_searxng(&raw, 10);
        let snippet = norm["results"][0]["snippet"].as_str().unwrap();
        assert_eq!(snippet.chars().count(), 500);
    }

    #[test]
    fn normalize_carries_image_fields() {
        let raw = json!({
            "results": [{
                "title": "rust",
                "url": "https://x.test",
                "content": "",
                "engine": "google images",
                "img_src": "https://x.test/img.png",
                "thumbnail_src": "https://x.test/thumb.png"
            }]
        });
        let norm = normalize_searxng(&raw, 10);
        let r = &norm["results"][0];
        assert_eq!(r["img_src"], "https://x.test/img.png");
        assert_eq!(r["thumbnail_src"], "https://x.test/thumb.png");
    }
}
