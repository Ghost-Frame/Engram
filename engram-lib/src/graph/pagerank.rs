use super::types::{GraphEdge, GraphNode};
use crate::Result;

pub fn compute_pagerank(
    nodes: &mut Vec<GraphNode>,
    edges: &[GraphEdge],
    damping: f32,
    iterations: u32,
) -> Result<()> {
    todo!()
}
