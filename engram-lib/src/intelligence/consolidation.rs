use crate::db::Database;
use crate::memory::types::Memory;
use crate::Result;
use serde::Serialize;

pub async fn consolidate(_db: &Database, _memory_ids: &[String]) -> Result<Memory> {
    todo!()
}

pub async fn find_consolidation_candidates(
    _db: &Database,
    _threshold: f32,
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
