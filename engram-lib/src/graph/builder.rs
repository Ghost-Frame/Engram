use super::types::{GraphEdge, GraphNode};
use super::types::GraphBuildOptions;
use crate::db::Database;
use crate::Result;
use serde::{Deserialize, Serialize};

pub async fn build_graph(_db: &Database) -> Result<(Vec<GraphNode>, Vec<GraphEdge>)> {
    todo!()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphBuildResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

pub async fn build_graph_data(
    db: &Database,
    _opts: &GraphBuildOptions,
) -> Result<GraphBuildResult> {
    let (nodes, edges) = build_graph(db).await?;
    Ok(GraphBuildResult { nodes, edges })
}
