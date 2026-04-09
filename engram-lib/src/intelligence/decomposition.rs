//! Decomposition -- break complex memories into atomic facts.

use crate::db::Database;
use crate::Result;

pub async fn decompose(_db: &Database, _memory_id: i64, _user_id: i64) -> Result<Vec<i64>> {
    Ok(Vec::new())
}
