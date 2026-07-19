use axum::{extract::State, routing::post, Json, Router};
use kleos_lib::memory::{
    self,
    search::hybrid_search,
    types::{SearchRequest, StoreRequest},
};
use kleos_lib::webhooks::resolve_and_validate_url;
use rusqlite::params;
use serde_json::{json, Value};

use crate::{
    error::AppError,
    extractors::{Auth, ResolvedDb},
    state::AppState,
};

mod types;
use types::FetchBody;

/// Maximum response body size for /fetch (10 MiB).
const FETCH_MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Shared HTTP client for /fetch endpoint -- no-redirect policy prevents SSRF
/// via open redirect chains after initial URL validation.
static FETCH_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Kleos/5.8 (fetch)")
        .pool_max_idle_per_host(4)
        .build()
        .expect("reqwest::Client::builder failed at onboard fetch client startup")
});

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/onboard", post(onboard))
        .route("/fetch", post(fetch_url))
}

async fn onboard(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
) -> Result<Json<Value>, AppError> {
    let mut checks: Vec<(&str, bool, String)> = Vec::new();

    // Test store
    let store_result = memory::store(
        &db,
        StoreRequest {
            content: "Kleos onboarding test memory -- safe to delete".into(),
            category: "system".into(),
            source: "onboarding".into(),
            user_id: Some(auth.effective_user_id()),
            ..Default::default()
        },
        None,
        false,
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
            if let Some(embedder) = state.current_embedder().await {
                embedder.embed("onboarding test").await.ok()
            } else {
                None
            }
        };
        let search_result = hybrid_search(
            &db,
            SearchRequest {
                query: "onboarding test".into(),
                embedding,
                limit: Some(1),
                user_id: Some(auth.effective_user_id()),
                ..Default::default()
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
        match memory::delete(&db, id, auth.effective_user_id()).await {
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
    let uid = auth.effective_user_id();
    let space_count: i64 = db
        .read(move |conn| {
            let count = conn.query_row(
                "SELECT COUNT(*) FROM spaces WHERE user_id = ?1",
                params![uid],
                |row| row.get::<_, i64>(0),
            )?;
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
            "Subscribe to live events via Axon: POST /axon/subscribe { agent, channel, webhook_url }",
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

async fn fetch_url(
    State(state): State<AppState>,
    Auth(auth): Auth,
    ResolvedDb(db): ResolvedDb,
    Json(body): Json<FetchBody>,
) -> Result<Json<Value>, AppError> {
    if body.url.trim().is_empty() {
        return Err(AppError(kleos_lib::EngError::InvalidInput(
            "'url' is required".into(),
        )));
    }

    // SECURITY (SSRF-DNS): validate URL scheme, literal hostname, AND resolve
    // DNS to reject domains that point at private/loopback/metadata IPs.
    // Pin the fetch to the validated IP to close the TOCTOU rebinding window.
    let pinned_ip = resolve_and_validate_url(&body.url).await.map_err(|e| {
        AppError(kleos_lib::EngError::InvalidInput(format!(
            "URL rejected: {}",
            e
        )))
    })?;

    let parsed = url::Url::parse(&body.url)
        .map_err(|_| AppError(kleos_lib::EngError::InvalidInput("Invalid URL".into())))?;

    let (fetch_url, host_override) = kleos_lib::webhooks::pin_url_to_ip(&body.url, pinned_ip);

    let mut req = FETCH_CLIENT.get(&fetch_url);
    if let Some(host) = &host_override {
        req = req.header("Host", host);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError(kleos_lib::EngError::Internal(format!("Fetch error: {}", e))))?;

    if !resp.status().is_success() {
        return Err(AppError(kleos_lib::EngError::Internal(format!(
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
                AppError(kleos_lib::EngError::Internal(format!("Read error: {}", e)))
            })?;
            if buf.len() + chunk.len() > FETCH_MAX_BODY_BYTES {
                return Err(AppError(kleos_lib::EngError::InvalidInput(format!(
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
        kleos_lib::ingestion::parsers::html::strip_tags(&raw)
    } else {
        raw.trim().to_string()
    };

    let content_len = content.len();

    // Optionally cache as memory
    let mut cached_id: Option<i64> = None;
    if body.cache.unwrap_or(false) && !content.is_empty() {
        let max_content = 50000;
        let store_content = if content.len() > max_content {
            kleos_lib::validation::truncate_on_char_boundary(&content, max_content)
        } else {
            &content
        };

        let mut req = StoreRequest {
            content: store_content.to_string(),
            category: "reference".into(),
            source: "fetch".into(),
            importance: 3,
            user_id: Some(auth.effective_user_id()),
            tags: Some(vec![format!(
                "url:{}",
                kleos_lib::validation::truncate_on_char_boundary(&body.url, 200)
            )]),
            ..Default::default()
        };

        if let Some(embedder) = state.current_embedder().await {
            if let Ok(emb) = embedder
                .embed(kleos_lib::validation::truncate_on_char_boundary(
                    store_content,
                    8000,
                ))
                .await
            {
                req.embedding = Some(emb);
            }
        }

        if let Ok(result) = memory::store(&db, req, None, false).await {
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

#[cfg(test)]
mod tests {
    use kleos_lib::webhooks::resolve_and_validate_url;

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
        assert!(resolve_and_validate_url("http://10.0.0.1/admin")
            .await
            .is_err());
        assert!(resolve_and_validate_url("http://192.168.1.1/admin")
            .await
            .is_err());
        assert!(resolve_and_validate_url("http://172.16.0.1/admin")
            .await
            .is_err());
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
            Ok(_) => {}
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
