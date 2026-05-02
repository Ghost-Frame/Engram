use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkType {
    Cite,
    Mentions,
    Contradicts,
    Refines,
    Generalizes,
    HasFact,
    Association,
    Temporal,
    Causal,
    Resolves,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLink {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub link_type: LinkType,
    pub weight: f32,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub weight: f32,
    pub pagerank: Option<f32>,
    pub community: Option<u32>,
    pub metadata: Option<serde_json::Value>,
    // Fields expected by engram-gui graph visualization
    #[serde(rename = "type")]
    pub node_type: String,
    pub category: String,
    pub importance: i64,
    pub group: String,
    pub size: f32,
    pub source: String,
    pub created_at: String,
    pub is_static: bool,
    pub content: String,
    pub source_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub link_type: LinkType,
    pub weight: f32,
}

// -- Entity types (used by entities.rs) --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: i64,
    pub name: String,
    pub entity_type: String,
    pub description: Option<String>,
    pub aliases: Option<String>,
    pub user_id: i64,
    pub space_id: Option<i64>,
    pub confidence: f64,
    pub occurrence_count: i64,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRelationship {
    pub id: i64,
    pub source_entity_id: i64,
    pub target_entity_id: i64,
    pub relationship_type: String,
    pub strength: f64,
    pub evidence_count: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMemorySearchResult {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: String,
    pub importance: i32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEntityRequest {
    pub name: String,
    pub entity_type: Option<String>,
    pub description: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub user_id: Option<i64>,
    pub space_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRelationshipRequest {
    pub source_entity_id: i64,
    pub target_entity_id: i64,
    pub relationship_type: Option<String>,
    pub strength: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GraphBuildOptions {
    #[serde(default)]
    pub user_id: i64,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphBuildResult {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunitiesResult {
    pub communities: usize,
    pub memories: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityMember {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub importance: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityStats {
    pub community_id: i64,
    pub count: i64,
    pub avg_importance: f64,
    pub categories: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRankResult {
    pub scores: HashMap<i64, f64>,
    pub iterations: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageRankUpdateResult {
    pub memories: usize,
    pub iterations: u32,
}

// -- Structural analysis engine types --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ENAction {
    pub subject: String,
    pub action: String,
    pub needs: Vec<String>,
    pub yields: Vec<String>,
    pub subsystem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub label: String,
    pub subject: Option<String>,
    pub subsystem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TopologyType {
    Pipeline,
    Tree,
    DAG,
    #[serde(rename = "Fork-Join")]
    ForkJoin,
    #[serde(rename = "Series-Parallel")]
    SeriesParallel,
    Cycle,
    Disconnected,
    #[serde(rename = "Single-Node")]
    SingleNode,
    Empty,
}

impl std::fmt::Display for TopologyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TopologyType::Pipeline => write!(f, "Pipeline"),
            TopologyType::Tree => write!(f, "Tree"),
            TopologyType::DAG => write!(f, "DAG"),
            TopologyType::ForkJoin => write!(f, "Fork-Join"),
            TopologyType::SeriesParallel => write!(f, "Series-Parallel"),
            TopologyType::Cycle => write!(f, "Cycle"),
            TopologyType::Disconnected => write!(f, "Disconnected"),
            TopologyType::SingleNode => write!(f, "Single-Node"),
            TopologyType::Empty => write!(f, "Empty"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeRole {
    SOURCE,
    SINK,
    HUB,
    FORK,
    JOIN,
    PIPELINE,
    CYCLE,
    ISOLATED,
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeRole::SOURCE => write!(f, "SOURCE"),
            NodeRole::SINK => write!(f, "SINK"),
            NodeRole::HUB => write!(f, "HUB"),
            NodeRole::FORK => write!(f, "FORK"),
            NodeRole::JOIN => write!(f, "JOIN"),
            NodeRole::PIPELINE => write!(f, "PIPELINE"),
            NodeRole::CYCLE => write!(f, "CYCLE"),
            NodeRole::ISOLATED => write!(f, "ISOLATED"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRoleInfo {
    pub id: String,
    pub label: String,
    pub role: NodeRole,
    #[serde(rename = "inDegree")]
    pub in_degree: usize,
    #[serde(rename = "outDegree")]
    pub out_degree: usize,
    pub subsystem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bridge {
    pub source: String,
    pub target: String,
    #[serde(rename = "sourceLabel")]
    pub source_label: String,
    #[serde(rename = "targetLabel")]
    pub target_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub topology: TopologyType,
    pub node_count: usize,
    pub edge_count: usize,
    pub nodes: Vec<NodeRoleInfo>,
    pub bridges: Vec<Bridge>,
    pub sources: Vec<String>,
    pub sinks: Vec<String>,
    pub hubs: Vec<String>,
    pub components: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthLevel {
    pub depth: usize,
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeImplication {
    pub bridge: Bridge,
    pub disconnected_components: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetailResult {
    pub topology: TopologyType,
    pub critical_path: Vec<String>,
    pub critical_path_length: usize,
    pub max_parallelism: usize,
    pub depth_levels: Vec<DepthLevel>,
    pub bridges: Vec<Bridge>,
    pub bridge_implications: Vec<BridgeImplication>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CentralityEntry {
    pub id: String,
    pub label: String,
    pub centrality: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BetweennessResult {
    pub node: String,
    pub label: String,
    pub centrality: f64,
    pub all_centralities: Vec<CentralityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathNode {
    pub id: String,
    pub label: String,
    pub subsystem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistanceResult {
    pub from: String,
    pub to: String,
    pub distance: Option<usize>,
    pub path: Option<Vec<PathNode>>,
    pub subsystem_crossings: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TracePathNode {
    pub id: String,
    pub label: String,
    pub role: NodeRole,
    pub subsystem: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverseEdge {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceResult {
    pub from: String,
    pub to: String,
    pub directed_path: Option<Vec<TracePathNode>>,
    pub undirected_fallback: bool,
    pub reverse_edges: Vec<ReverseEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactResult {
    pub removed_node: String,
    pub removed_label: String,
    pub original_components: usize,
    pub after_components: usize,
    pub disconnected_nodes: Vec<String>,
    pub topology_before: TopologyType,
    pub topology_after: TopologyType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleChange {
    pub node: String,
    pub role_a: NodeRole,
    pub role_b: NodeRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    pub topology_a: TopologyType,
    pub topology_b: TopologyType,
    pub topology_changed: bool,
    pub nodes_only_in_a: Vec<String>,
    pub nodes_only_in_b: Vec<String>,
    pub nodes_in_both: Vec<String>,
    pub role_changes: Vec<RoleChange>,
    pub edge_count_a: usize,
    pub edge_count_b: usize,
    pub bridge_count_a: usize,
    pub bridge_count_b: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolveResult {
    pub diff: DiffResult,
    pub new_bridges: Vec<Bridge>,
    pub eliminated_bridges: Vec<Bridge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsystemInfo {
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorizeResult {
    pub subsystems: Vec<SubsystemInfo>,
    pub modularity: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractResult {
    pub subsystem: String,
    pub source: String,
    pub boundary_inputs: Vec<String>,
    pub boundary_outputs: Vec<String>,
    pub internal_entities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeResult {
    pub merged_source: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub linked_entities: Vec<String>,
}

pub struct MemoryRecord {
    pub id: i64,
    pub content: String,
    pub category: String,
    pub source: Option<String>,
}

pub struct LinkRecord {
    pub source_id: i64,
    pub target_id: i64,
    pub link_type: String,
    pub similarity: f64,
}
