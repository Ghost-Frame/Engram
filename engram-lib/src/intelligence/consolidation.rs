use crate::db::Database;
use crate::Result;
use crate::memory::types::Memory;

pub async fn consolidate(db: &Database, memory_ids: &[String]) -> Result<Memory> {
    todo!()
}

pub async fn find_consolidation_candidates(
    db: &Database,
    threshold: f32,
) -> Result<Vec<Vec<String>>> {
    todo!()
}
