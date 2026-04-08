// GROUNDING SEARCH - Coordinated search across providers (ported from TS grounding/search.ts)
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::warn;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult { pub source: String, pub title: String, pub content: String, pub url: Option<String>, pub score: f64, pub metadata: Option<serde_json::Value> }

#[derive(Default)]
pub struct SearchCoordinator { providers: Vec<Box<dyn SearchProvider + Send + Sync>> }

pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &str;
    fn search(&self, query: &str, limit: usize) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<SearchResult>> + Send + '_>>;
}

impl SearchCoordinator {
    pub fn new() -> Self { Self::default() }
    pub fn register_provider(&mut self, provider: Box<dyn SearchProvider + Send + Sync>) { self.providers.push(provider); }

    pub async fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        if self.providers.is_empty() { return Vec::new(); }
        let mut all = Vec::new();
        for provider in &self.providers {
            match std::panic::AssertUnwindSafe(provider.search(query, limit)).await {
                results => all.extend(results),
            }
        }
        // Dedup by url or content prefix
        let mut seen = HashSet::new();
        let mut deduped = Vec::new();
        for r in all {
            let key = r.url.clone().unwrap_or_else(|| r.content.chars().take(100).collect());
            if seen.insert(key) { deduped.push(r); }
        }
        deduped.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        deduped.truncate(limit);
        deduped
    }

    pub fn list_providers(&self) -> Vec<&str> { self.providers.iter().map(|p| p.name()).collect() }
}
