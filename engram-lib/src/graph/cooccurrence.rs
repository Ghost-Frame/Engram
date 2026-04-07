use super::types::GraphEdge;
use crate::db::Database;
use crate::Result;

pub async fn build_cooccurrence_edges(
    db: &Database,
    window_size: usize,
) -> Result<Vec<GraphEdge>> {
    todo!()
}
