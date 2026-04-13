use axum::{extract::State, routing::post, Json, Router};
use engram_lib::memory::{
    self,
    search::hybrid_search,
    types::{SearchRequest, StoreRequest},
};
use engram_lib::webhooks::resolve_and_validate_url;
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

/// Maximum response body size for /fetch (10 MiB).
const FETCH_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/onboard", post(onboard))
        .route("/fetch", post(fetch_url))
}

async fn onboard(State(state): State<AppState>, Auth(auth): Auth) -> Result<Json<Value>, AppError> {
    let mut checks: Vec<(&str, bool, String)> = Vec::new();

    // Test store
    let store_result = memory::store(
        &state.db,
        StoreRequest {
            content: "Engram onboarding test memory -- safe to delete".into(),
            category: "system".into(),
            source: "onboarding".into(),
            importance: 5,
            user_id: Some(auth.user_id),
            tags: None,
            embedding: None,
            session_id: None,
            is_static: None,
            space_id: None,
            parent_memory_id: None,
        },
    )
    .await;

    let test_id = match store_result {
        Ok(result) => {
            checks.push((
                "store",
                true,
                format!("Created test memory id={}", result.id),
            ));
            Some(result.id)
        }
        Err(e) => {
            checks.push(("store", false, e.to_string()));
            None
        }
    };

    // Test search
    if test_id.is_some() {
        let embedding = {
            let embedder_guard = state.embedder.read().await;
            if let Some(ref embedder) = *embedder_guard {
                embedder.embed("onboarding test").await.ok()
            } else {
                None
            }
        };
        let search_result = hybrid_search(
            &state.db,
            SearchRequest {
                query: "onboarding test".into(),
                embedding,
                limit: Some(1),
                user_id: Some(auth.user_id),
                latest_only: true,
                category: None,
                source: None,
                tags: None,
                threshold: None,
                space_id: None,
                include_forgotten: None,
                mode: None,
                question_type: None,
                expand_relationships: false,
                include_links: false,
                source_filter: None,
            },
        )
        .await;
        match search_result {
            Ok(results) => {
                checks.push((
                    "search",
                    true,
                    format!("Search returned {} results", results.len()),
                ));
            }
            Err(e) => {
                checks.push(("search", false, e.to_string()));
            }
        }
    }

    // Cleanup test memory
    if let Some(id) = test_id {
        match memory::delete(&state.db, id, auth.user_id).await {
            Ok(()) => checks.push(("cleanup", true, "Test memory deleted".into())),
            Err(e) => checks.push(("cleanup", false, e.to_string())),
        }
    }

    // Check embedding
    let embedder_ready = state.embedder.read().await.is_some();
    checks.push((
        "embedding",
        embedder_ready,
        if embedder_ready {
            "Embedding provider ready"
        } else {
            "Embedding provider loading or unavailable"
        }
        .into(),
    ));

    // Check spaces
    let uid = auth.user_id;
    let space_count: i64 = state
        .db
        .read(move |conn| {
            let count = conn
                .query_row(
                    "SELECT COUNT(*) FROM spaces WHERE user_id = ?1",
                    params![uid],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(count)
        })
        .await
        .unwrap_or(0);
    checks.push((
        "spaces",
        space_count > 0,
        format!("{} space(s) configured", space_count),
    ));

    let all_passed = checks.iter().all(|(_, passed, _)| *passed);
    let checks_json: Value = checks
        .iter()
        .map(|(name, passed, detail)| {
            (
                name.to_string(),
                json!({ "passed": passed, "detail": detail }),
            )
        })
        .collect::<serde_json::Map<String, Value>>()
        .into();

    let next_steps = if all_passed {
        vec![
            "Store your first real memory: POST /store { content: '...' }",
            "Search for it: POST /search { query: '...' }",
            "Set up a webhook for events: POST /webhooks { url: '...', events: ['*'] }",
        ]
    } else {
        vec!["Fix the failed checks above, then run POST /onboard again"]
    };

    Ok(Json(json!({
        "status": if all_passed { "ready" } else { "issues_found" },
        "checks": checks_json,
        "next_steps": next_steps,
    })))
}

#[derive(Debug, Deserialize)]
struct FetchBody {
    pub url: String,
    pub cache: Option<bool>,
}

async fn fetch_url(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<FetchBody>,
) -> Result<Json<Value>, AppError> {
    if body.url.trim().is_empty() {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "'url' is required".into(),
        )));
    }

    // SECURITY (SSRF-DNS): validate URL scheme, literal hostname, AND resolve
    // DNS to reject domains that point at private/loopback/metadata IPs. This
    // closes the DNS-rebinding SSRF gap where a public domain resolves to
    // 127.0.0.1, 169.254.169.254, RFC1918 space, etc.
    resolve_and_validate_url(&body.url).await.map_err(|e| {
        AppError(engram_lib::EngError::InvalidInput(format!(
            "URL rejected: {}",
            e
        )))
    })?;

    let parsed = url::Url::parse(&body.url)
        .map_err(|_| AppError(engram_lib::EngError::InvalidInput("Invalid URL".into())))?;

    // Disable redirect following to prevent SSRF via open redirects that
    // bounce to internal hosts after the initial validation.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Engram/5.8 (fetch)")
        .build()
        .map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))?;

    let resp = client.get(&body.url).send().await.map_err(|e| {
        AppError(engram_lib::EngError::Internal(format!(
            "Fetch error: {}",
            e
        )))
    })?;

    if !resp.status().is_success() {
        return Err(AppError(engram_lib::EngError::Internal(format!(
            "Fetch failed: {} {}",
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("")
        ))));
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // SECURITY (DoS): stream body with a hard byte ceiling instead of
    // buffering the entire upstream response. Reject once exceeded.
    let raw = {
        use futures::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                AppError(engram_lib::EngError::Internal(format!("Read error: {}", e)))
            })?;
            if buf.len() + chunk.len() > FETCH_MAX_BODY_BYTES {
                return Err(AppError(engram_lib::EngError::InvalidInput(format!(
                    "Response body exceeds {} byte limit",
                    FETCH_MAX_BODY_BYTES
                ))));
            }
            buf.extend_from_slice(&chunk);
        }
        String::from_utf8_lossy(&buf).into_owned()
    };

    let mut title = parsed.host_str().unwrap_or("unknown").to_string();
    let content = if content_type.contains("html") {
        // Extract title
        if let Some(cap) = raw.find("<title").and_then(|start| {
            let after = &raw[start..];
            let content_start = after.find('>')? + 1;
            let content_end = after.find("</title>")?;
            Some(after[content_start..content_end].trim().to_string())
        }) {
            title = cap;
        }
        // Simple HTML to text: strip tags
        strip_html_tags(&raw)
    } else {
        raw.trim().to_string()
    };

    let content_len = content.len();

    // Optionally cache as memory
    let mut cached_id: Option<i64> = None;
    if body.cache.unwrap_or(false) && !content.is_empty() {
        let max_content = 50000;
        let store_content = if content.len() > max_content {
            &content[..max_content]
        } else {
            &content
        };

        let mut req = StoreRequest {
            content: store_content.to_string(),
            category: "reference".into(),
            source: "fetch".into(),
            importance: 3,
            user_id: Some(auth.user_id),
            tags: Some(vec![format!(
                "url:{}",
                &body.url[..body.url.len().min(200)]
            )]),
            embedding: None,
            session_id: None,
            is_static: None,
            space_id: None,
            parent_memory_id: None,
        };

        {
            let embedder_guard = state.embedder.read().await;
            if let Some(ref embedder) = *embedder_guard {
                if let Ok(emb) = embedder
                    .embed(&store_content[..store_content.len().min(8000)])
                    .await
                {
                    req.embedding = Some(emb);
                }
            }
        }

        if let Ok(result) = memory::store(&state.db, req).await {
            cached_id = Some(result.id);
        }
    }

    Ok(Json(json!({
        "content": content,
        "title": title,
        "url": body.url,
        "length": content_len,
        "cached_id": cached_id,
    })))
}

/// Minimal HTML tag stripper. Removes script/style blocks, then strips remaining tags.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let chars = html.chars();
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_buf = String::new();

    for c in chars {
        if c == '<' {
            in_tag = true;
            tag_buf.clear();
            continue;
        }
        if in_tag {
            if c == '>' {
                in_tag = false;
                let lower = tag_buf.to_lowercase();
                if lower.starts_with("script") {
                    in_script = true;
                } else if lower.starts_with("/script") {
                    in_script = false;
                } else if lower.starts_with("style") {
                    in_style = true;
                } else if lower.starts_with("/style") {
                    in_style = false;
                }
                tag_buf.clear();
            } else {
                tag_buf.push(c);
            }
            continue;
        }
        if !in_script && !in_style {
            result.push(c);
        }
    }

    // Collapse whitespace
    let mut collapsed = String::with_capacity(result.len());
    let mut prev_newline = false;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_newline {
                collapsed.push('\n');
                prev_newline = true;
            }
        } else {
            collapsed.push_str(trimmed);
            collapsed.push('\n');
            prev_newline = false;
        }
    }

    collapsed.trim().to_string()
}

#[cfg(test)]
mod tests {
    use engram_lib::webhooks::resolve_and_validate_url;

    /// Regression: /fetch previously only checked literal hostname strings.
    /// A public domain resolving to 127.0.0.1 bypassed the check entirely.
    /// Now resolve_and_validate_url is called, which resolves DNS first.

    #[tokio::test]
    async fn fetch_rejects_localhost_literal() {
        let r = resolve_and_validate_url("https://127.0.0.1/secret").await;
        assert!(r.is_err(), "literal 127.0.0.1 must be rejected");
    }

    #[tokio::test]
    async fn fetch_rejects_localhost_domain() {
        let r = resolve_and_validate_url("https://localhost/secret").await;
        assert!(r.is_err(), "localhost domain must be rejected");
    }

    #[tokio::test]
    async fn fetch_rejects_metadata_ip() {
        let r = resolve_and_validate_url("http://169.254.169.254/latest/meta-data").await;
        assert!(r.is_err(), "metadata IP must be rejected");
    }

    #[tokio::test]
    async fn fetch_rejects_private_rfc1918() {
        assert!(resolve_and_validate_url("http://10.0.0.1/admin").await.is_err());
        assert!(resolve_and_validate_url("http://192.168.1.1/admin").await.is_err());
        assert!(resolve_and_validate_url("http://172.16.0.1/admin").await.is_err());
    }

    #[tokio::test]
    async fn fetch_rejects_ipv6_loopback() {
        let r = resolve_and_validate_url("http://[::1]/secret").await;
        assert!(r.is_err(), "IPv6 loopback must be rejected");
    }

    #[tokio::test]
    async fn fetch_rejects_ipv4_mapped_ipv6() {
        let r = resolve_and_validate_url("http://[::ffff:127.0.0.1]/secret").await;
        assert!(r.is_err(), "IPv4-mapped IPv6 loopback must be rejected");
    }

    #[tokio::test]
    async fn fetch_rejects_cgnat_range() {
        let r = resolve_and_validate_url("http://100.64.0.1/internal").await;
        assert!(r.is_err(), "CGNAT 100.64/10 must be rejected");
    }

    #[tokio::test]
    async fn fetch_rejects_ftp_scheme() {
        let r = resolve_and_validate_url("ftp://evil.com/file").await;
        assert!(r.is_err(), "non-http(s) schemes must be rejected");
    }

    #[tokio::test]
    async fn fetch_accepts_public_https() {
        // May fail if DNS is down, which is fine -- we just skip.
        match resolve_and_validate_url("https://example.com/page").await {
            Ok(()) => {}
            Err(e) => {
                let msg = format!("{}", e);
                if msg.contains("DNS resolution failed") {
                    return; // DNS unavailable, skip
                }
                panic!("unexpected rejection of public URL: {}", e);
            }
        }
    }
}
