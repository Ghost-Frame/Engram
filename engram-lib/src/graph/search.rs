use super::types::GraphNode;
use crate::db::Database;
use crate::Result;

pub async fn graph_search(
    _db: &Database,
    _query: &str,
    _limit: usize,
) -> Result<Vec<GraphNode>> {
    todo!()
}

pub async fn neighborhood(
    _db: &Database,
    _node_id: &str,
    _depth: u32,
) -> Result<(Vec<GraphNode>, Vec<super::types::GraphEdge>)> {
    todo!()
}
