use super::types::GraphNode;
use crate::db::Database;
use crate::Result;

pub async fn graph_search(
    db: &Database,
    query: &str,
    limit: usize,
) -> Result<Vec<GraphNode>> {
    todo!()
}

pub async fn neighborhood(
    db: &Database,
    node_id: &str,
    depth: u32,
) -> Result<(Vec<GraphNode>, Vec<super::types::GraphEdge>)> {
    todo!()
}
