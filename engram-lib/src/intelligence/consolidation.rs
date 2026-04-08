use crate::db::Database;
use crate::memory::types::Memory;
use crate::Result;
use serde::Serialize;

pub async fn consolidate(db: &Database, memory_ids: &[String]) -> Result<Memory> {
    todo!()
}

pub async fn find_consolidation_candidates(
    db: &Database,
    threshold: f32,
) -> Result<Vec<Vec<String>>> {
    todo!()
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsolidationRecord {
    pub id: i64,
    pub summary: String,
}

pub async fn list_consolidations(
    _db: &Database,
    _user_id: i64,
    _limit: usize,
) -> Result<Vec<ConsolidationRecord>> {
    Ok(Vec::new())
}
