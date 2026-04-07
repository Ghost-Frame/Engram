use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::db::Database;
use crate::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub memory_ids: Vec<String>,
    pub project: Option<String>,
    pub tags: Vec<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn create_episode(db: &Database, episode: Episode) -> Result<Episode> {
    todo!()
}

pub async fn get_episode(db: &Database, id: &str) -> Result<Episode> {
    todo!()
}

pub async fn list_episodes(db: &Database, limit: usize, offset: usize) -> Result<Vec<Episode>> {
    todo!()
}

pub async fn update_episode(db: &Database, id: &str, episode: Episode) -> Result<Episode> {
    todo!()
}

pub async fn delete_episode(db: &Database, id: &str) -> Result<()> {
    todo!()
}
