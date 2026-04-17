use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

const DEFAULT_OPENSPACE_API_URL: &str = "https://api.openspace.dev";
const TIMEOUT_SECS: u64 = 10;

/// Shared HTTP client for cloud skill registry calls.
static CLOUD_CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS + 5))
        .pool_max_idle_per_host(2)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});

/// Stubbed cloud skill candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudSearchResult {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub origin: String,
    pub tags: Vec<String>,
    pub score: f64,
}

/// Stub: Search skills in the cloud registry.
#[tracing::instrument(skip(query), fields(query_len = query.len(), limit))]
pub async fn search_skills_cloud(query: &str, limit: usize) -> Result<Vec<CloudSearchResult>> {
    let Some(api_key) = std::env::var("OPENSPACE_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok(vec![]);
    };
    let base_url = std::env::var("OPENSPACE_API_URL")
        .unwrap_or_else(|_| DEFAULT_OPENSPACE_API_URL.to_string());
    let url = format!("{}/skills/search", base_url.trim_end_matches('/'));
    let response = CLOUD_CLIENT
        .get(url)
        .bearer_auth(api_key)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
        .query(&[("q", query), ("limit", &limit.to_string())])
        .send()
        .await;

    let Ok(response) = response else {
        return Ok(vec![]);
    };
    if !response.status().is_success() {
        return Ok(vec![]);
    }

    let payload: serde_json::Value = response.json().await.unwrap_or_else(|_| json!([]));
    let results = payload.get("results").cloned().unwrap_or(payload);
    Ok(serde_json::from_value(results).unwrap_or_default())
}

/// Stub: Upload a skill to the cloud registry.
#[tracing::instrument(skip(description, content, tags), fields(name = %name, category = %category, tags_count = tags.len(), content_len = content.len()))]
pub async fn upload_skill_to_cloud(
    name: &str,
    description: &str,
    content: &str,
    category: &str,
    tags: &[String],
) -> Result<String> {
    let api_key = std::env::var("OPENSPACE_API_KEY")
        .map_err(|_| crate::EngError::InvalidInput("OPENSPACE_API_KEY not configured".into()))?;
    let base_url = std::env::var("OPENSPACE_API_URL")
        .unwrap_or_else(|_| DEFAULT_OPENSPACE_API_URL.to_string());
    let response = CLOUD_CLIENT
        .post(format!("{}/skills", base_url.trim_end_matches('/')))
        .bearer_auth(api_key)
        .json(&json!({
            "name": name,
            "description": description,
            "content": content,
            "category": category,
            "tags": tags,
        }))
        .send()
        .await
        .map_err(|e| crate::EngError::Internal(format!("cloud upload failed: {}", e)))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(crate::EngError::Internal(format!(
            "cloud upload failed ({}): {}",
            status, body
        )));
    }

    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| crate::EngError::Internal(format!("invalid cloud upload response: {}", e)))?;
    Ok(payload
        .get("skill_id")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("id").and_then(|v| v.as_str()))
        .unwrap_or_default()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn test_search_returns_empty() {
        let results = search_skills_cloud("test", 10).await.unwrap();
        assert!(results.is_empty());
    }
    #[tokio::test]
    async fn test_upload_returns_error() {
        let result = upload_skill_to_cloud("test", "desc", "content", "workflow", &[]).await;
        assert!(result.is_err());
    }
}
