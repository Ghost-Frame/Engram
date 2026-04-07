use crate::db::Database;
use crate::Result;
use crate::memory::types::Memory;

#[derive(Debug, Clone)]
pub struct Contradiction {
    pub memory_a: String,
    pub memory_b: String,
    pub confidence: f32,
    pub description: String,
}

pub async fn detect_contradictions(
    db: &Database,
    memory: &Memory,
) -> Result<Vec<Contradiction>> {
    todo!()
}

pub async fn scan_all_contradictions(db: &Database) -> Result<Vec<Contradiction>> {
    todo!()
}
