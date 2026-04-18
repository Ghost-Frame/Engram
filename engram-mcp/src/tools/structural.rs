use crate::auth::resolve_auth;
use crate::tools::{with_auth_props, ToolDef};
use crate::{invalid_input, App};
use engram_lib::graph::structural::{
    analyze_memory_graph, analyze_system, categorize_system, compose_systems, compute_betweenness,
    compute_distance, compute_impact, detail_analysis, evolve_system, extract_subsystem,
    structural_diff, trace_flow,
};
use engram_lib::graph::types::{LinkRecord, MemoryRecord};
use engram_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef {
            name: "structural.analyze",
            description: "Structural analysis of a system described in EN syntax. Returns topology classification (Pipeline, Tree, Fork-Join, DAG, Cycle, Disconnected), node roles (SOURCE, SINK, FORK, JOIN, HUB, PIPELINE), and bridges (single points of failure). EN syntax: Subject do: action needs: inputs yields: outputs.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"}
            }), &["source"]),
        },
        ToolDef {
            name: "structural.detail",
            description: "Deep structural analysis -- concurrency metrics, critical path, flow depth levels, resilience analysis with bridge implications.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"}
            }), &["source"]),
        },
        ToolDef {
            name: "structural.between",
            description: "Betweenness centrality for a node -- what fraction of all shortest paths flow through it. Score 0-1.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"},
                "node": {"type":"string","description":"Node name to compute centrality for"}
            }), &["source","node"]),
        },
        ToolDef {
            name: "structural.distance",
            description: "Shortest path between two nodes with subsystem crossing annotations.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"},
                "from": {"type":"string","description":"Starting node name"},
                "to": {"type":"string","description":"Target node name"}
            }), &["source","from","to"]),
        },
        ToolDef {
            name: "structural.trace",
            description: "Follow directed flow from node A to node B respecting yields->needs direction. Falls back to undirected and flags reverse edges.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"},
                "from": {"type":"string","description":"Starting node name"},
                "to": {"type":"string","description":"Target node name"}
            }), &["source","from","to"]),
        },
        ToolDef {
            name: "structural.impact",
            description: "Blast radius -- remove a node and see what disconnects. Works for any domain: infra, org charts, compliance flows.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"},
                "node": {"type":"string","description":"Node to remove for impact analysis"}
            }), &["source","node"]),
        },
        ToolDef {
            name: "structural.diff",
            description: "Structural diff between two systems. Reports topology changes, role changes, nodes added/removed, bridge count changes.",
            input_schema: || with_auth_props(json!({
                "source_a": {"type":"string","description":"EN source for the first system"},
                "source_b": {"type":"string","description":"EN source for the second system"}
            }), &["source_a","source_b"]),
        },
        ToolDef {
            name: "structural.evolve",
            description: "Dry-run architectural changes. Apply a patch and see the structural delta plus new/eliminated bridges.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source for the current system"},
                "patch": {"type":"string","description":"EN source patch to apply"}
            }), &["source","patch"]),
        },
        ToolDef {
            name: "structural.categorize",
            description: "Auto-discover subsystem boundaries from dependency structure using Louvain community detection.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"}
            }), &["source"]),
        },
        ToolDef {
            name: "structural.extract",
            description: "Extract a named subsystem as standalone EN source. Reports boundary inputs, outputs, and internal entities.",
            input_schema: || with_auth_props(json!({
                "source": {"type":"string","description":"EN source code describing the system"},
                "subsystem": {"type":"string","description":"Name of the subsystem to extract"}
            }), &["source","subsystem"]),
        },
        ToolDef {
            name: "structural.compose",
            description: "Merge two EN graphs into one with entity linking.",
            input_schema: || with_auth_props(json!({
                "source_a": {"type":"string","description":"EN source for the first system"},
                "source_b": {"type":"string","description":"EN source for the second system"},
                "links": {"type":"string","description":"Entity links: 'a.node1=b.node2, a.node3=b.node4'"}
            }), &["source_a","source_b"]),
        },
        ToolDef {
            name: "structural.memory_graph",
            description: "Analyze Engram's own memory link graph structurally. Returns topology, node roles, bridges, and metrics.",
            input_schema: || with_auth_props(json!({
                "limit": {"type":"integer","description":"Max memories to include (default: 500)"}
            }), &[]),
        },
    ]);
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.analyze"))]
pub async fn analyze(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    Ok(json!(analyze_system(source)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.detail"))]
pub async fn detail(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    Ok(json!(detail_analysis(source)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.between"))]
pub async fn between(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    let node = args
        .get("node")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("node required"))?;
    Ok(json!(compute_betweenness(source, node)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.distance"))]
pub async fn distance(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    let from = args
        .get("from")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("from required"))?;
    let to = args
        .get("to")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("to required"))?;
    Ok(json!(compute_distance(source, from, to)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.trace"))]
pub async fn trace(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    let from = args
        .get("from")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("from required"))?;
    let to = args
        .get("to")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("to required"))?;
    Ok(json!(trace_flow(source, from, to)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.impact"))]
pub async fn impact(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    let node = args
        .get("node")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("node required"))?;
    Ok(json!(compute_impact(source, node)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.diff"))]
pub async fn diff(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source_a = args
        .get("source_a")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source_a required"))?;
    let source_b = args
        .get("source_b")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source_b required"))?;
    Ok(json!(structural_diff(source_a, source_b)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.evolve"))]
pub async fn evolve(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    let patch = args
        .get("patch")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("patch required"))?;
    Ok(json!(evolve_system(source, patch)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.categorize"))]
pub async fn categorize(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    Ok(json!(categorize_system(source)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.extract"))]
pub async fn extract(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source required"))?;
    let subsystem = args
        .get("subsystem")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("subsystem required"))?;
    Ok(json!(extract_subsystem(source, subsystem)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.compose"))]
pub async fn compose(app: &App, args: Value) -> Result<Value> {
    let _auth = resolve_auth(app, &args).await?;
    let source_a = args
        .get("source_a")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source_a required"))?;
    let source_b = args
        .get("source_b")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("source_b required"))?;
    let links = args
        .get("links")
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok(json!(compose_systems(source_a, source_b, links)))
}

#[tracing::instrument(skip(app, args), fields(tool = "structural.memory_graph"))]
pub async fn memory_graph(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(500);
    let user_id = auth.user_id;

    let memories: Vec<MemoryRecord> = app
        .db
        .read(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT id, content, category, source FROM memories \
                     WHERE user_id = ?1 AND is_forgotten = 0 \
                     ORDER BY id DESC LIMIT ?2",
                )
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
            let rows = stmt
                .query_map(rusqlite::params![user_id, limit], |row| {
                    Ok(MemoryRecord {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        source: row.get(3)?,
                    })
                })
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
            Ok(rows)
        })
        .await?;

    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
    let ids_for_query = ids.clone();

    let links: Vec<LinkRecord> = if ids_for_query.is_empty() {
        vec![]
    } else {
        app.db
            .read(move |conn| {
                // Fetch all links where both source and target are in our memory set.
                // SQLite does not support binding arrays, so we fetch all links for the user
                // and filter in Rust.
                let mut stmt = conn
                    .prepare(
                        "SELECT ml.source_id, ml.target_id, ml.type, ml.similarity \
                         FROM memory_links ml \
                         INNER JOIN memories ms ON ms.id = ml.source_id \
                         WHERE ms.user_id = ?1 \
                         LIMIT 5000",
                    )
                    .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
                let id_set: std::collections::HashSet<i64> = ids_for_query.into_iter().collect();
                let rows = stmt
                    .query_map(rusqlite::params![user_id], |row| {
                        Ok(LinkRecord {
                            source_id: row.get(0)?,
                            target_id: row.get(1)?,
                            link_type: row.get(2)?,
                            similarity: row.get(3)?,
                        })
                    })
                    .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| engram_lib::EngError::DatabaseMessage(e.to_string()))?;
                Ok(rows
                    .into_iter()
                    .filter(|l| id_set.contains(&l.source_id) && id_set.contains(&l.target_id))
                    .collect::<Vec<_>>())
            })
            .await?
    };

    Ok(json!(analyze_memory_graph(&memories, &links)))
}
