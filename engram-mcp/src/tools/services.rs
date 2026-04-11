use crate::auth::resolve_auth;
use crate::tools::{with_auth_props, ToolDef};
use crate::{invalid_input, App};
use engram_lib::services::{axon, broca, chiasm, soma, thymus};
use engram_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef { name: "services.axon_publish", description: "Publish an Axon event.", input_schema: || with_auth_props(json!({
            "channel":{"type":"string"},"action":{"type":"string"},"payload":{},"source":{"type":"string"},"agent":{"type":"string"}
        }), &["channel"]) },
        ToolDef { name: "services.axon_consume", description: "Query Axon events.", input_schema: || with_auth_props(json!({
            "channel":{"type":"string"},"action":{"type":"string"},"source":{"type":"string"},"limit":{"type":"integer"},"offset":{"type":"integer"}
        }), &[]) },
        ToolDef { name: "services.broca_log", description: "Log a Broca action.", input_schema: || with_auth_props(json!({
            "agent":{"type":"string"},"service":{"type":"string"},"action":{"type":"string"},"narrative":{"type":"string"},"payload":{},"axon_event_id":{"type":"integer"}
        }), &["agent"]) },
        ToolDef { name: "services.chiasm_create_task", description: "Create a Chiasm task.", input_schema: || with_auth_props(json!({
            "agent":{"type":"string"},"project":{"type":"string"},"title":{"type":"string"},"status":{"type":"string"},"summary":{"type":"string"}
        }), &["agent","project","title"]) },
        ToolDef { name: "services.chiasm_update_task", description: "Update a Chiasm task.", input_schema: || with_auth_props(json!({
            "id":{"type":"integer"},"title":{"type":"string"},"status":{"type":"string"},"summary":{"type":"string"},"agent":{"type":"string"}
        }), &["id"]) },
        ToolDef { name: "services.soma_register", description: "Register a Soma agent.", input_schema: || with_auth_props(json!({
            "name":{"type":"string"},"type":{"type":"string"},"description":{"type":"string"},"capabilities":{},"config":{}
        }), &["name","type"]) },
        ToolDef { name: "services.soma_heartbeat", description: "Heartbeat a Soma agent.", input_schema: || with_auth_props(json!({"id":{"type":"integer"}}), &["id"]) },
        ToolDef { name: "services.thymus_review", description: "Record a Thymus evaluation.", input_schema: || with_auth_props(json!({
            "rubric_id":{"type":"integer"},"agent":{"type":"string"},"subject":{"type":"string"},"input":{},"output":{},"scores":{},"notes":{"type":"string"},"evaluator":{"type":"string"}
        }), &["rubric_id","agent","subject","scores","evaluator"]) },
    ]);
}

pub async fn axon_publish(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!(axon::publish_event(&app.db, axon::PublishEventRequest {
        channel: args.get("channel").and_then(Value::as_str).ok_or_else(|| invalid_input("channel required"))?.to_string(),
        action: args.get("action").and_then(Value::as_str).or_else(|| args.get("event_type").and_then(Value::as_str)).unwrap_or("event").to_string(),
        payload: args.get("payload").cloned(),
        source: args.get("source").and_then(Value::as_str).map(str::to_string),
        agent: args.get("agent").and_then(Value::as_str).map(str::to_string),
        user_id: Some(auth.user_id),
    }).await?))
}

pub async fn axon_consume(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!({
        "events": axon::query_events(
            &app.db,
            args.get("channel").and_then(Value::as_str),
            args.get("action").and_then(Value::as_str),
            args.get("source").and_then(Value::as_str),
            args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize,
            args.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize,
            auth.user_id,
        ).await?
    }))
}

pub async fn broca_log(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!(broca::log_action(&app.db, broca::LogActionRequest {
        agent: args.get("agent").and_then(Value::as_str).ok_or_else(|| invalid_input("agent required"))?.to_string(),
        service: args.get("service").and_then(Value::as_str).map(str::to_string),
        action: args.get("action").and_then(Value::as_str).unwrap_or("unknown").to_string(),
        narrative: args.get("narrative").and_then(Value::as_str).map(str::to_string),
        payload: args.get("payload").cloned(),
        axon_event_id: args.get("axon_event_id").and_then(Value::as_i64),
        user_id: Some(auth.user_id),
    }).await?))
}

pub async fn chiasm_create_task(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!(chiasm::create_task(&app.db, chiasm::CreateTaskRequest {
        agent: args.get("agent").and_then(Value::as_str).ok_or_else(|| invalid_input("agent required"))?.to_string(),
        project: args.get("project").and_then(Value::as_str).ok_or_else(|| invalid_input("project required"))?.to_string(),
        title: args.get("title").and_then(Value::as_str).ok_or_else(|| invalid_input("title required"))?.to_string(),
        status: args.get("status").and_then(Value::as_str).map(str::to_string),
        summary: args.get("summary").and_then(Value::as_str).map(str::to_string),
        user_id: Some(auth.user_id),
    }).await?))
}

pub async fn chiasm_update_task(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args.get("id").and_then(Value::as_i64).ok_or_else(|| invalid_input("id required"))?;
    Ok(json!(chiasm::update_task(&app.db, id, chiasm::UpdateTaskRequest {
        title: args.get("title").and_then(Value::as_str).map(str::to_string),
        status: args.get("status").and_then(Value::as_str).map(str::to_string),
        summary: args.get("summary").and_then(Value::as_str).map(str::to_string),
        agent: args.get("agent").and_then(Value::as_str).map(str::to_string),
    }, auth.user_id).await?))
}

pub async fn soma_register(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!(soma::register_agent(&app.db, soma::RegisterAgentRequest {
        name: args.get("name").and_then(Value::as_str).ok_or_else(|| invalid_input("name required"))?.to_string(),
        type_: args.get("type").and_then(Value::as_str).ok_or_else(|| invalid_input("type required"))?.to_string(),
        description: args.get("description").and_then(Value::as_str).map(str::to_string),
        capabilities: args.get("capabilities").cloned(),
        config: args.get("config").cloned(),
        user_id: Some(auth.user_id),
    }).await?))
}

pub async fn soma_heartbeat(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let id = args.get("id").and_then(Value::as_i64).ok_or_else(|| invalid_input("id required"))?;
    soma::heartbeat(&app.db, id, auth.user_id).await?;
    Ok(json!({"ok": true, "id": id}))
}

pub async fn thymus_review(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    Ok(json!(thymus::evaluate(&app.db, thymus::EvaluateRequest {
        rubric_id: args.get("rubric_id").and_then(Value::as_i64).ok_or_else(|| invalid_input("rubric_id required"))?,
        agent: args.get("agent").and_then(Value::as_str).ok_or_else(|| invalid_input("agent required"))?.to_string(),
        subject: args.get("subject").and_then(Value::as_str).ok_or_else(|| invalid_input("subject required"))?.to_string(),
        input: args.get("input").cloned(),
        output: args.get("output").cloned(),
        scores: args.get("scores").cloned().ok_or_else(|| invalid_input("scores required"))?,
        notes: args.get("notes").and_then(Value::as_str).map(str::to_string),
        evaluator: args.get("evaluator").and_then(Value::as_str).ok_or_else(|| invalid_input("evaluator required"))?.to_string(),
        user_id: Some(auth.user_id),
    }).await?))
}
