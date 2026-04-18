pub mod admin;
pub mod context;
pub mod graph;
pub mod intelligence;
pub mod memory;
pub mod services;
pub mod skills;
pub mod structural;

use crate::App;
use engram_lib::Result;
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: fn() -> Value,
}

fn schema(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": true
    })
}

pub fn auth_prop() -> Value {
    json!({
        "bearer_token": { "type": "string", "description": "Optional bearer token override. Defaults to ENGRAM_MCP_BEARER_TOKEN." }
    })
}

pub fn registry() -> Vec<Value> {
    all_tools()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": (tool.input_schema)(),
            })
        })
        .collect()
}

fn all_tools() -> Vec<ToolDef> {
    let mut out = Vec::new();
    memory::register(&mut out);
    context::register(&mut out);
    graph::register(&mut out);
    intelligence::register(&mut out);
    services::register(&mut out);
    admin::register(&mut out);
    structural::register(&mut out);
    skills::register(&mut out);
    out
}

#[tracing::instrument(skip(app, args), fields(name = %name))]
pub async fn dispatch(app: &App, name: &str, args: Value) -> Result<Value> {
    match name {
        "memory.store" => memory::store(app, args).await,
        "memory.search" => memory::search(app, args).await,
        "memory.get" => memory::get(app, args).await,
        "memory.list" => memory::list(app, args).await,
        "memory.update" => memory::update(app, args).await,
        "memory.delete" => memory::delete(app, args).await,
        "memory.mark_forgotten" => memory::mark_forgotten(app, args).await,
        "memory.mark_archived" => memory::mark_archived(app, args).await,
        "memory.mark_unarchived" => memory::mark_unarchived(app, args).await,
        "memory.update_forget_reason" => memory::update_forget_reason(app, args).await,
        "memory.adjust_importance" => memory::adjust_importance(app, args).await,
        "memory.insert_link" => memory::insert_link(app, args).await,
        "memory.get_by_content_hash" => memory::get_by_content_hash(app, args).await,
        "context.assemble_context" => context::assemble_context(app, args).await,
        "context.get_header" => context::get_header(app, args).await,
        "context.generate_prompt" => context::generate_prompt(app, args).await,
        "graph.get_neighbors" => graph::get_neighbors(app, args).await,
        "graph.pagerank_top" => graph::pagerank_top(app, args).await,
        "graph.louvain_communities" => graph::louvain_communities(app, args).await,
        "graph.cooccurrence" => graph::cooccurrence(app, args).await,
        "intelligence.consolidate" => intelligence::consolidate(app, args).await,
        "intelligence.detect_contradictions" => {
            intelligence::detect_contradictions(app, args).await
        }
        "intelligence.decompose" => intelligence::decompose(app, args).await,
        "intelligence.temporal_summary" => intelligence::temporal_summary(app, args).await,
        "intelligence.reflect" => intelligence::reflect(app, args).await,
        "intelligence.extract_facts" => intelligence::extract_facts(app, args).await,
        "intelligence.sentiment" => intelligence::sentiment(app, args).await,
        "intelligence.causal_trace" => intelligence::causal_trace(app, args).await,
        "services.axon_publish" => services::axon_publish(app, args).await,
        "services.axon_consume" => services::axon_consume(app, args).await,
        "services.broca_log" => services::broca_log(app, args).await,
        "services.chiasm_create_task" => services::chiasm_create_task(app, args).await,
        "services.chiasm_update_task" => services::chiasm_update_task(app, args).await,
        "services.soma_register" => services::soma_register(app, args).await,
        "services.soma_heartbeat" => services::soma_heartbeat(app, args).await,
        "services.thymus_review" => services::thymus_review(app, args).await,
        "admin.reembed" => admin::reembed(app, args).await,
        "admin.rebuild_fts" => admin::rebuild_fts(app, args).await,
        "admin.vector_sync_replay" => admin::vector_sync_replay(app, args).await,
        "admin.backup" => admin::backup(app, args).await,
        "admin.checkpoint" => admin::checkpoint(app, args).await,
        "structural.analyze" => structural::analyze(app, args).await,
        "structural.detail" => structural::detail(app, args).await,
        "structural.between" => structural::between(app, args).await,
        "structural.distance" => structural::distance(app, args).await,
        "structural.trace" => structural::trace(app, args).await,
        "structural.impact" => structural::impact(app, args).await,
        "structural.diff" => structural::diff(app, args).await,
        "structural.evolve" => structural::evolve(app, args).await,
        "structural.categorize" => structural::categorize(app, args).await,
        "structural.extract" => structural::extract(app, args).await,
        "structural.compose" => structural::compose(app, args).await,
        "structural.memory_graph" => structural::memory_graph(app, args).await,
        "skill.search" => skills::skill_search(app, args).await,
        "skill.fix" => skills::skill_fix(app, args).await,
        "skill.upload" => skills::skill_upload(app, args).await,
        "skill.execute" => skills::skill_execute(app, args).await,
        _ => Err(engram_lib::EngError::NotFound(format!(
            "unknown tool: {name}"
        ))),
    }
}

pub fn with_auth_props(extra: Value, required: &[&str]) -> Value {
    let mut properties = auth_prop().as_object().cloned().unwrap_or_default();
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        properties.insert(key, value);
    }
    schema(Value::Object(properties), required)
}
