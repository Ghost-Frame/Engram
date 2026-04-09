use axum::{
    extract::State,
    routing::post,
    Json, Router,
};
use engram_lib::memory::{self, search::hybrid_search, types::{SearchRequest, StoreRequest}};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{error::AppError, extractors::Auth, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/onboard", post(onboard))
        .route("/fetch", post(fetch_url))
}

async fn onboard(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<Value>, AppError> {
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
            checks.push(("store", true, format!("Created test memory id={}", result.id)));
            Some(result.id)
        }
        Err(e) => {
            checks.push(("store", false, e.to_string()));
            None
        }
    };

    // Test search
    if test_id.is_some() {
        let embedding = if let Some(ref embedder) = state.embedder {
            embedder.embed("onboarding test").await.ok()
        } else {
            None
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
                checks.push(("search", true, format!("Search returned {} results", results.len())));
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
    checks.push(("embedding", state.embedder.is_some(), if state.embedder.is_some() { "Embedding provider ready" } else { "No embedding provider configured" }.into()));

    // Check spaces
    let space_count = {
        let mut rows = state
            .db
            .conn
            .query(
                "SELECT COUNT(*) FROM spaces WHERE user_id = ?1",
                libsql::params![auth.user_id],
            )
            .await
            .unwrap_or_else(|_| unreachable!());
        match rows.next().await {
            Ok(Some(r)) => r.get::<i64>(0).unwrap_or(0),
            _ => 0,
        }
    };
    checks.push(("spaces", space_count > 0, format!("{} space(s) configured", space_count)));

    let all_passed = checks.iter().all(|(_, passed, _)| *passed);
    let checks_json: Value = checks
        .iter()
        .map(|(name, passed, detail)| {
            (name.to_string(), json!({ "passed": passed, "detail": detail }))
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

    let parsed = url::Url::parse(&body.url).map_err(|_| {
        AppError(engram_lib::EngError::InvalidInput("Invalid URL".into()))
    })?;

    // Only allow http/https
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError(engram_lib::EngError::InvalidInput(
            "Only http/https URLs allowed".into(),
        )));
    }

    // Block private/internal addresses
    if let Some(host) = parsed.host_str() {
        let h = host.to_lowercase();
        if h == "localhost"
            || h == "127.0.0.1"
            || h == "::1"
            || h == "0.0.0.0"
            || h.starts_with("10.")
            || h.starts_with("192.168.")
            || h.starts_with("172.16.")
            || h.starts_with("172.17.")
            || h.starts_with("172.18.")
            || h.starts_with("172.19.")
            || h.starts_with("172.2")
            || h.starts_with("172.30.")
            || h.starts_with("172.31.")
            || h.ends_with(".local")
            || h.ends_with(".internal")
            || h.starts_with("100.64.")
            || h.starts_with("169.254.")
            || h.starts_with("fc")
            || h.starts_with("fd")
        {
            return Err(AppError(engram_lib::EngError::InvalidInput(
                "URL cannot point to private/internal addresses".into(),
            )));
        }
    }

    // Fetch the URL
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent("Engram/5.8 (fetch)")
        .build()
        .map_err(|e| AppError(engram_lib::EngError::Internal(e.to_string())))?;

    let resp = client
        .get(&body.url)
        .send()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Internal(format!("Fetch error: {}", e))))?;

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

    let raw = resp
        .text()
        .await
        .map_err(|e| AppError(engram_lib::EngError::Internal(format!("Read error: {}", e))))?;

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
            tags: Some(vec![format!("url:{}", &body.url[..body.url.len().min(200)])]),
            embedding: None,
            session_id: None,
            is_static: None,
            space_id: None,
            parent_memory_id: None,
        };

        if let Some(ref embedder) = state.embedder {
            if let Ok(emb) = embedder.embed(&store_content[..store_content.len().min(8000)]).await {
                req.embedding = Some(emb);
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
