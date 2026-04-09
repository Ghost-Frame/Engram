use crate::db::Database;
use crate::memory::types::Memory;
use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Contradiction {
    pub memory_a: String,
    pub memory_b: String,
    pub confidence: f32,
    pub description: String,
}

pub async fn detect_contradictions(
    _db: &Database,
    _memory: &Memory,
) -> Result<Vec<Contradiction>> {
    todo!()
}

pub async fn scan_all_contradictions(_db: &Database) -> Result<Vec<Contradiction>> {
    todo!()
}
