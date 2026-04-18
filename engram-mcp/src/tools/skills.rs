use crate::auth::resolve_auth;
use crate::tools::{with_auth_props, ToolDef};
use crate::{invalid_input, App};
use engram_lib::skills::{self, create_skill, evolver, search::search_skills, CreateSkillRequest};
use engram_lib::Result;
use serde_json::{json, Value};

pub fn register(out: &mut Vec<ToolDef>) {
    out.extend([
        ToolDef {
            name: "skill.search",
            description: "Search for skills across the local registry. Keyword + semantic search. Use to find relevant skills before executing a task.",
            input_schema: || with_auth_props(json!({
                "query": {"type":"string","description":"Search query (natural language or keywords)"},
                "limit": {"type":"integer","description":"Max results (default: 20)"}
            }), &["query"]),
        },
        ToolDef {
            name: "skill.fix",
            description: "Fix a broken skill using LLM-driven patching. Provide the skill ID and a description of what to fix.",
            input_schema: || with_auth_props(json!({
                "skill_id": {"type":"integer","description":"Skill ID to fix"},
                "direction": {"type":"string","description":"What is broken and how to fix it. Be specific."}
            }), &["skill_id"]),
        },
        ToolDef {
            name: "skill.upload",
            description: "Create a skill from name and content and store it in the local registry.",
            input_schema: || with_auth_props(json!({
                "name": {"type":"string","description":"Skill name"},
                "code": {"type":"string","description":"Skill content (markdown or code)"},
                "description": {"type":"string","description":"Human-readable description"},
                "language": {"type":"string","description":"Language/format (default: markdown)"},
                "tags": {"type":"array","items":{"type":"string"},"description":"Tags for categorization"},
                "agent": {"type":"string","description":"Agent name (default: mcp)"}
            }), &["name","code"]),
        },
        ToolDef {
            name: "skill.execute",
            description: "Search for relevant skills matching a task and return them as context. Live LLM execution requires the engram-server; this tool returns matched skill content for use as guidance.",
            input_schema: || with_auth_props(json!({
                "task": {"type":"string","description":"The task instruction (natural language)"},
                "limit": {"type":"integer","description":"Max skills to retrieve (default: 5)"}
            }), &["task"]),
        },
    ]);
}

#[tracing::instrument(skip(app, args), fields(tool = "skill.search"))]
pub async fn skill_search(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("query required"))?;
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize;
    let results = search_skills(&app.db, query, auth.user_id, limit).await?;
    Ok(json!({"results": results, "count": results.len()}))
}

#[tracing::instrument(skip(app, args), fields(tool = "skill.fix"))]
pub async fn skill_fix(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let skill_id = args
        .get("skill_id")
        .and_then(Value::as_i64)
        .ok_or_else(|| invalid_input("skill_id required"))?;
    // Verify ownership before calling fix.
    skills::get_skill(&app.db, skill_id, auth.user_id).await?;
    let result = evolver::fix_skill(&app.db, skill_id, "mcp", auth.user_id).await?;
    Ok(json!(result))
}

#[tracing::instrument(skip(app, args), fields(tool = "skill.upload"))]
pub async fn skill_upload(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("name required"))?;
    let code = args
        .get("code")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("code required"))?;
    let req = CreateSkillRequest {
        name: name.to_string(),
        agent: args
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("mcp")
            .to_string(),
        description: args
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string),
        code: code.to_string(),
        language: Some(
            args.get("language")
                .and_then(Value::as_str)
                .unwrap_or("markdown")
                .to_string(),
        ),
        parent_skill_id: None,
        metadata: None,
        user_id: Some(auth.user_id),
        tags: args
            .get("tags")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        tool_deps: None,
    };
    let skill = create_skill(&app.db, req).await?;
    Ok(json!({"created": true, "skill": skill}))
}

#[tracing::instrument(skip(app, args), fields(tool = "skill.execute"))]
pub async fn skill_execute(app: &App, args: Value) -> Result<Value> {
    let auth = resolve_auth(app, &args).await?;
    let task = args
        .get("task")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_input("task required"))?;
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(5) as usize;
    // No LLM wired in engram-mcp; return matched skills as context for the caller to use.
    let results = search_skills(&app.db, task, auth.user_id, limit).await?;
    let skill_names: Vec<String> = results.iter().map(|s| s.name.clone()).collect();
    let context_parts: Vec<Value> = results
        .iter()
        .map(|s| {
            json!({
                "id": s.id,
                "name": s.name,
                "description": s.description,
                "code": s.code,
                "trust_score": s.trust_score,
            })
        })
        .collect();
    Ok(json!({
        "task": task,
        "skills_found": results.len(),
        "skills_used": skill_names,
        "skill_context": context_parts,
        "note": "Live LLM execution is not available in the MCP crate. Use skill_context as guidance."
    }))
}
