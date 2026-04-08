use crate::Result;
use serde::{Deserialize, Serialize};

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
pub async fn search_skills_cloud(
    _query: &str,
    _limit: usize,
) -> Result<Vec<CloudSearchResult>> {
    // TODO: Wire to cloud skill registry via reqwest
    Ok(vec![])
}

/// Stub: Upload a skill to the cloud registry.
pub async fn upload_skill_to_cloud(
    _name: &str,
    _description: &str,
    _content: &str,
    _category: &str,
    _tags: &[String],
) -> Result<String> {
    Err(crate::EngError::Internal("cloud upload not yet wired".into()))
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