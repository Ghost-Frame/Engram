//! Route registry. One entry per HTTP route on `kleos-server`. Both the
//! `kleos-mcp` tool list and the runtime dispatcher iterate this slice -- so
//! adding a new route means a single new entry here, nothing else.

use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde_json::Value;

/// Characters to encode in path segments: everything except RFC 3986 unreserved chars.
/// Crucially includes `/`, `?`, `#`, `%` so callers cannot inject path or query structure.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'@')
    .add(b':')
    .add(b'[')
    .add(b']');

/// HTTP method for a registered route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}

/// Required scope on the server side. Surfaced for documentation; the server
/// is the authoritative enforcer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Read,
    Write,
    Admin,
}

/// One MCP tool == one kleos-server HTTP route.
///
/// `path` is a template; `{name}` segments are filled from the corresponding
/// top-level field of the call arguments before the request goes out, and the
/// field is dropped from the JSON body so it does not duplicate the path.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    /// Canonical tool name, e.g. "memory.store".
    pub name: &'static str,
    /// Back-compat aliases. Same dispatch target.
    pub aliases: &'static [&'static str],
    pub method: Method,
    pub path: &'static str,
    pub scope: Scope,
    pub description: &'static str,
    /// Raw JSON Schema literal for the tool's input. Parsed once at startup.
    pub input_schema: &'static str,
}

/// Walks the registry and returns the route whose canonical name or aliases match.
pub fn find_by_name(name: &str) -> Option<&'static Route> {
    ROUTES
        .iter()
        .find(|r| r.name == name || r.aliases.contains(&name))
}

/// Canonical names of routes that must never be reachable over MCP. These
/// resolve or proxy raw credential values, so dispatching them through the
/// MCP/model-context channel would land plaintext secrets in transcripts and
/// logs. They remain available over the authenticated HTTP API, which is their
/// intended surface.
pub const MCP_BLOCKED_ROUTES: &[&str] = &["admin.cred_resolve", "admin.cred_proxy"];

/// Returns true if `name` (canonical or alias) resolves to a route that is
/// blocked from MCP dispatch. Both MCP dispatchers consult this before calling
/// a tool so an unlisted-but-dispatchable secret route cannot be invoked by
/// name.
pub fn is_mcp_blocked(name: &str) -> bool {
    find_by_name(name)
        .map(|r| MCP_BLOCKED_ROUTES.contains(&r.name))
        .unwrap_or(false)
}

/// Substitutes `{key}` segments in the template with values from `args`,
/// removing those keys from `args` so they do not duplicate in the body.
pub fn render_path(template: &str, args: &mut Value) -> Result<String, String> {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 1..];
        let close = after_open
            .find('}')
            .ok_or_else(|| format!("malformed path template: {template}"))?;
        let key = &after_open[..close];
        let value = args
            .as_object_mut()
            .and_then(|m| m.remove(key))
            .ok_or_else(|| format!("missing path argument '{key}' for template {template}"))?;
        match value {
            Value::String(s) => {
                out.push_str(&utf8_percent_encode(&s, PATH_SEGMENT).to_string());
            }
            Value::Number(n) => {
                // Numbers are already safe (digits, sign, dot) -- no encoding needed.
                out.push_str(&n.to_string());
            }
            Value::Bool(b) => {
                out.push_str(if b { "true" } else { "false" });
            }
            other => {
                return Err(format!(
                    "path argument '{key}' must be a string, number, or bool (got {})",
                    other
                ));
            }
        }
        rest = &after_open[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Shorthand for declaring a route entry in the registry.
macro_rules! route {
    // No aliases
    ($method:ident, $scope:ident, $name:expr, $path:expr, $desc:expr, $schema:expr) => {
        Route { name: $name, aliases: &[], method: Method::$method, path: $path, scope: Scope::$scope, description: $desc, input_schema: $schema }
    };
    // With aliases
    ($method:ident, $scope:ident, $name:expr, $path:expr, $desc:expr, [$($alias:expr),+ $(,)?], $schema:expr) => {
        Route { name: $name, aliases: &[$($alias),+], method: Method::$method, path: $path, scope: Scope::$scope, description: $desc, input_schema: $schema }
    };
}

/// The route registry. Hand-maintained for now; one entry per HTTP route.
///
/// Entries here drive both the MCP `tools/list` payload and the dispatcher.
/// The kleos-cli aliases on memory/skills/etc preserve back-compat with
/// existing MCP client configurations that may reference the old names.
pub static ROUTES: &[Route] = &[
    // -- memory -----------------------------------------------------------
    route!(
        Post,
        Write,
        "memory.store",
        "/store",
        "Store a new memory.",
        ["memory_store"],
        r#"{"type":"object","properties":{"content":{"type":"string"},"category":{"type":"string"},"source":{"type":"string"},"importance":{"type":"integer"},"tags":{"type":"array","items":{"type":"string"}},"session_id":{"type":"string"},"is_static":{"type":"boolean"},"space_id":{"type":"integer"}},"required":["content"]}"#
    ),
    route!(
        Post,
        Read,
        "memory.search",
        "/search",
        "Hybrid search across memories.",
        ["memory_search", "memory_search_preset"],
        r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"},"category":{"type":"string"},"source":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"threshold":{"type":"number"},"space_id":{"type":"integer"},"include_forgotten":{"type":"boolean"},"mode":{"type":"string"},"question_type":{"type":"string"},"expand_relationships":{"type":"boolean"},"include_links":{"type":"boolean"},"latest_only":{"type":"boolean"},"source_filter":{"type":"string"}},"required":["query"]}"#
    ),
    route!(
        Get,
        Read,
        "memory.get",
        "/memory/{id}",
        "Fetch a memory by id.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "memory.list",
        "/list",
        "Paginated listing of memories.",
        ["memory_list"],
        r#"{"type":"object","properties":{"limit":{"type":"integer"},"offset":{"type":"integer"},"category":{"type":"string"},"source":{"type":"string"},"space_id":{"type":"integer"},"include_forgotten":{"type":"boolean"},"include_archived":{"type":"boolean"}}}"#
    ),
    route!(
        Post,
        Write,
        "memory.update",
        "/memory/{id}/update",
        "Update a memory.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"content":{"type":"string"},"category":{"type":"string"},"source":{"type":"string"},"importance":{"type":"integer"},"tags":{"type":"array","items":{"type":"string"}},"status":{"type":"string"}},"required":["id"]}"#
    ),
    route!(
        Delete,
        Write,
        "memory.delete",
        "/memory/{id}",
        "Delete a memory.",
        ["memory_delete"],
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "memory.mark_forgotten",
        "/memory/{id}/forget",
        "Soft-delete a memory.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"reason":{"type":"string"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "memory.mark_archived",
        "/memory/{id}/archive",
        "Archive a memory.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "memory.mark_unarchived",
        "/memory/{id}/unarchive",
        "Restore an archived memory.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Read,
        "memory.recall",
        "/recall",
        "Recall by query, returns ranked memories.",
        ["memory_recall"],
        r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]}"#
    ),
    // -- skills -----------------------------------------------------------
    route!(
        Post,
        Read,
        "skill.search",
        "/skills/search",
        "Search skill registry.",
        ["skill_search"],
        r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"},"tags":{"type":"array","items":{"type":"string"}}},"required":["query"]}"#
    ),
    route!(
        Post,
        Read,
        "skill.execute",
        "/skills/execute",
        "Match skills to a task and return the most relevant skill content.",
        ["skill_execute"],
        r#"{"type":"object","properties":{"task":{"type":"string"},"limit":{"type":"integer"}},"required":["task"]}"#
    ),
    route!(
        Post,
        Write,
        "skill.upload",
        "/skills/capture",
        "Create a new skill record.",
        ["skill_upload"],
        r#"{"type":"object","properties":{"name":{"type":"string"},"code":{"type":"string"},"language":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"agent":{"type":"string"}},"required":["name","code"]}"#
    ),
    route!(
        Post,
        Write,
        "skill.fix",
        "/skills/{skill_id}/fix",
        "Run LLM-driven repair on a skill.",
        ["skill_fix"],
        r#"{"type":"object","properties":{"skill_id":{"type":"integer"},"direction":{"type":"string"}},"required":["skill_id"]}"#
    ),
    // -- activity ---------------------------------------------------------
    route!(
        Post,
        Write,
        "activity.report",
        "/activity",
        "Unified fan-out hub (Chiasm, Axon, Broca, Thymus, Skills, Memory).",
        r#"{"type":"object","properties":{"action":{"type":"string"},"summary":{"type":"string"},"project":{"type":"string"},"agent":{"type":"string"},"metadata":{"type":"object"}},"required":["action","summary"]}"#
    ),
    // -- structural -------------------------------------------------------
    // EN-syntax structural analysis. Mirrors the legacy snake_case names
    // as aliases so older MCP clients keep working unchanged.
    route!(
        Post,
        Read,
        "structural.analyze",
        "/structural/analyze",
        "Classify topology, node roles, and bridges for an EN-syntax system.",
        ["structural_analyze"],
        r#"{"type":"object","properties":{"source":{"type":"string"}},"required":["source"]}"#
    ),
    route!(
        Post,
        Read,
        "structural.detail",
        "/structural/detail",
        "Extended structural metrics: concurrency, critical path, flow depth, bridge implications.",
        ["structural_detail"],
        r#"{"type":"object","properties":{"source":{"type":"string"}},"required":["source"]}"#
    ),
    route!(
        Post,
        Read,
        "structural.between",
        "/structural/between",
        "Betweenness centrality for one node (fraction of shortest paths through it).",
        ["structural_between"],
        r#"{"type":"object","properties":{"source":{"type":"string"},"node":{"type":"string"}},"required":["source","node"]}"#
    ),
    route!(
        Post,
        Read,
        "structural.distance",
        "/structural/distance",
        "Directed shortest path between two nodes.",
        ["structural_distance"],
        r#"{"type":"object","properties":{"source":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"}},"required":["source","from","to"]}"#
    ),
    route!(
        Post,
        Read,
        "structural.trace",
        "/structural/trace",
        "Directed path with undirected fallback; flags reversed hops.",
        ["structural_trace"],
        r#"{"type":"object","properties":{"source":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"}},"required":["source","from","to"]}"#
    ),
    // -- health -----------------------------------------------------------
    route!(
        Get,
        Read,
        "health",
        "/health",
        "Server liveness probe.",
        r#"{"type": "object"}"#
    ),
    // -- handoffs ---------------------------------------------------------
    route!(
        Post,
        Write,
        "handoffs.store",
        "/handoffs",
        "Store a new handoff dump.",
        ["handoffs.dump"],
        r#"{"type":"object","properties":{"project":{"type":"string"},"content":{"type":"string"},"agent":{"type":"string"},"type":{"type":"string"},"branch":{"type":"string"},"directory":{"type":"string"},"session_id":{"type":"string"},"model":{"type":"string"},"host":{"type":"string"},"metadata":{"type":"object"},"atoms":{"type":"array","items":{"type":"object"}}},"required":["content","project"]}"#
    ),
    route!(
        Get,
        Read,
        "handoffs.list",
        "/handoffs",
        "List handoffs.",
        r#"{"type": "object", "properties": {"project": {"type":"string"}, "agent": {"type":"string"}, "type": {"type":"string"}, "model": {"type":"string"}, "session_id": {"type":"string"}, "host": {"type":"string"}, "since": {"type":"string"}, "limit": {"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "handoffs.latest",
        "/handoffs/latest",
        "Fetch the most recent handoff.",
        r#"{"type": "object", "properties": {"project": {"type":"string"}, "agent": {"type":"string"}, "type": {"type":"string"}, "model": {"type":"string"}, "session_id": {"type":"string"}, "host": {"type":"string"}, "since": {"type":"string"}, "limit": {"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "handoffs.search",
        "/handoffs/search",
        "Search handoffs by query string.",
        r#"{"type": "object", "properties": {"q": {"type":"string"}, "project": {"type":"string"}, "limit": {"type":"integer"}}, "required": ["q"]}"#
    ),
    route!(
        Get,
        Read,
        "handoffs.stats",
        "/handoffs/stats",
        "Aggregate handoff statistics.",
        r#"{"type": "object"}"#
    ),
    route!(
        Post,
        Write,
        "handoffs.gc",
        "/handoffs/gc",
        "Garbage-collect old handoffs.",
        r#"{"type": "object", "properties": {"tiered": {"type":"boolean"}, "keep": {"type":"integer"}}}"#
    ),
    route!(
        Delete,
        Write,
        "handoffs.delete",
        "/handoffs/{id}",
        "Delete a specific handoff.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    // -- jobs -------------------------------------------------------------
    route!(
        Get,
        Read,
        "jobs.list",
        "/jobs",
        "List background jobs.",
        r#"{"type": "object", "properties": {"limit": {"type":"integer"}, "status": {"type":"string"}}}"#
    ),
    route!(
        Get,
        Read,
        "jobs.pending",
        "/jobs/pending",
        "List pending jobs.",
        r#"{"type": "object"}"#
    ),
    route!(
        Get,
        Read,
        "jobs.running",
        "/jobs/running",
        "List currently running jobs.",
        r#"{"type": "object"}"#
    ),
    route!(
        Get,
        Read,
        "jobs.failed",
        "/jobs/failed",
        "List failed jobs.",
        r#"{"type": "object"}"#
    ),
    route!(
        Post,
        Write,
        "jobs.retry",
        "/jobs/retry",
        "Retry failed jobs (all matching).",
        r#"{"type": "object", "properties": {"all": {"type":"boolean"}, "kind": {"type":"string"}}}"#
    ),
    route!(
        Post,
        Write,
        "jobs.retry_by_id",
        "/jobs/{id}/retry",
        "Retry a specific job by id.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    route!(
        Post,
        Admin,
        "jobs.purge",
        "/jobs/purge",
        "Permanently remove jobs matching the criteria.",
        r#"{"type": "object", "properties": {"status": {"type":"string"}, "before": {"type":"string"}}}"#
    ),
    route!(
        Post,
        Admin,
        "jobs.cleanup",
        "/jobs/cleanup",
        "Garbage-collect completed jobs.",
        r#"{"type": "object", "properties": {"keep": {"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "jobs.stats",
        "/jobs/stats",
        "Job queue statistics.",
        r#"{"type": "object"}"#
    ),
    // -- fsrs (spaced repetition) -----------------------------------------
    route!(
        Post,
        Write,
        "fsrs.review",
        "/fsrs/review",
        "Record an FSRS review outcome for a memory.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"memory_id":{"type":"integer"},"grade":{"type":"integer","description":"1=Again, 2=Hard, 3=Good, 4=Easy"}}}"#
    ),
    route!(
        Get,
        Read,
        "fsrs.state",
        "/fsrs/state",
        "Fetch FSRS scheduling state for a memory.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    route!(
        Post,
        Admin,
        "fsrs.init",
        "/fsrs/init",
        "Backfill FSRS state for existing memories.",
        r#"{"type": "object", "properties": {"limit": {"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "fsrs.recall_due",
        "/fsrs/recall-due",
        "List memories due for spaced-repetition reinforcement.",
        r#"{"type": "object", "properties": {"topic": {"type":"string"}, "limit": {"type":"integer"}, "session": {"type":"string"}}, "required": ["topic"]}"#
    ),
    // -- tasks (chiasm) ---------------------------------------------------
    route!(
        Get,
        Read,
        "tasks.list",
        "/tasks",
        "List Chiasm coordination tasks.",
        r#"{"type": "object", "properties": {"agent": {"type":"string"}, "project": {"type":"string"}, "status": {"type":"string"}, "limit": {"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "tasks.create",
        "/tasks",
        "Create a Chiasm coordination task.",
        ["services.chiasm_create_task"],
        r#"{"type":"object","properties":{"agent":{"type":"string"},"project":{"type":"string"},"title":{"type":"string"},"summary":{"type":"string"},"status":{"type":"string"}},"required":["agent","project","title"]}"#
    ),
    route!(
        Get,
        Read,
        "tasks.stats",
        "/tasks/stats",
        "Aggregate task statistics.",
        r#"{"type": "object"}"#
    ),
    route!(
        Get,
        Read,
        "tasks.history",
        "/tasks/{id}/history",
        "Task status history.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    route!(
        Get,
        Read,
        "tasks.feed",
        "/feed",
        "Cross-task activity feed.",
        r#"{"type": "object", "properties": {"limit": {"type":"integer"}}}"#
    ),
    // -- sessions ---------------------------------------------------------
    route!(
        Get,
        Read,
        "sessions.get",
        "/sessions/{id}",
        "Fetch a session record.",
        r#"{"type": "object", "properties": {"id": {"type":"string"}}, "required": ["id"]}"#
    ),
    route!(
        Post,
        Write,
        "sessions.append",
        "/sessions/{id}/append",
        "Append an entry to a session.",
        r#"{"type":"object","properties":{"id":{"type":"string"},"role":{"type":"string"},"content":{"type":"string"},"metadata":{"type":"object"}},"required":["id","content"]}"#
    ),
    // -- scratchpad -------------------------------------------------------
    route!(
        Get,
        Read,
        "scratchpad.list",
        "/scratch",
        "List scratchpad entries.",
        r#"{"type": "object", "properties": {"session": {"type":"string"}}}"#
    ),
    route!(
        Put,
        Write,
        "scratchpad.put",
        "/scratch",
        "Store or update a scratchpad entry.",
        r#"{"type":"object","properties":{"session":{"type":"string"},"key":{"type":"string"},"value":{}},"required":["session","key","value"]}"#
    ),
    route!(
        Delete,
        Write,
        "scratchpad.delete_session",
        "/scratch/{session}",
        "Delete every scratchpad entry for a session.",
        r#"{"type": "object", "properties": {"session": {"type":"string"}}, "required": ["session"]}"#
    ),
    route!(
        Delete,
        Write,
        "scratchpad.delete_key",
        "/scratch/{session}/{key}",
        "Delete a specific scratchpad key.",
        r#"{"type": "object", "properties": {"session": {"type":"string"}, "key": {"type":"string"}}, "required": ["session", "key"]}"#
    ),
    route!(
        Post,
        Write,
        "scratchpad.promote",
        "/scratch/{session}/promote",
        "Promote a scratchpad session to durable memory.",
        r#"{"type": "object", "properties": {"session": {"type":"string"}}, "required": ["session"]}"#
    ),
    route!(
        Get,
        Read,
        "scratchpad.get",
        "/scratchpad/get",
        "Read one scratchpad ledger entry by namespace (agent) and key. Used by the ke edit-gate to verify spec-task coverage before allowing a file edit.",
        r#"{"type": "object", "properties": {"namespace": {"type":"string"}, "key": {"type":"string"}}, "required": ["namespace", "key"]}"#
    ),
    // -- episodes ---------------------------------------------------------
    route!(
        Get,
        Read,
        "episodes.list",
        "/episodes",
        "List episodes.",
        ["memory_episodes"],
        r#"{"type": "object", "properties": {"limit": {"type":"integer"}, "offset": {"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "episodes.create",
        "/episodes",
        "Create a new episode.",
        r#"{"type":"object","properties":{"title":{"type":"string"},"summary":{"type":"string"},"started_at":{"type":"string"},"metadata":{"type":"object"}},"required":["title"]}"#
    ),
    route!(
        Get,
        Read,
        "episodes.get",
        "/episodes/{id}",
        "Fetch one episode by id.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    route!(
        Patch,
        Write,
        "episodes.update",
        "/episodes/{id}",
        "Update an episode.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"title":{"type":"string"},"summary":{"type":"string"},"metadata":{"type":"object"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "episodes.assign_memories",
        "/episodes/{id}/memories",
        "Attach memories to an episode.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"memory_ids":{"type":"array","items":{"type":"integer"}}},"required":["id","memory_ids"]}"#
    ),
    route!(
        Post,
        Write,
        "episodes.finalize",
        "/episodes/{id}/finalize",
        "Close an episode and lock its membership.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    // -- prompts ----------------------------------------------------------
    route!(
        Get,
        Read,
        "prompts.get",
        "/prompt",
        "Generate a RAG-based prompt from memory.",
        r#"{"type":"object","properties":{"format":{"type":"string"},"tokens":{"type":"integer"},"context":{"type":"string"}}}"#
    ),
    route!(
        Post,
        Read,
        "prompts.generate",
        "/prompt/generate",
        "Pack memories into a prompt for a given task.",
        ["context.generate_prompt"],
        r#"{"type":"object","properties":{"task":{"type":"string"},"format":{"type":"string"},"limit":{"type":"integer"}},"required":["task"]}"#
    ),
    route!(
        Post,
        Read,
        "prompts.header",
        "/header",
        "Generate a task header from cross-agent activity.",
        ["context.get_header"],
        r#"{"type":"object","properties":{"task":{"type":"string"},"agents":{"type":"array","items":{"type":"string"}}}}"#
    ),
    // -- personality ------------------------------------------------------
    route!(
        Post,
        Read,
        "personality.detect",
        "/personality/detect",
        "Detect personality cues from a sample of text.",
        r#"{"type": "object", "properties": {"content": {"type":"string"}}, "required": ["content"]}"#
    ),
    route!(
        Get,
        Read,
        "personality.profile",
        "/personality/profile",
        "Fetch the current personality profile.",
        r#"{"type": "object"}"#
    ),
    route!(
        Post,
        Write,
        "personality.update_profile",
        "/personality/profile/update",
        "Update fields on the personality profile.",
        r#"{"type": "object", "properties": {"traits": {"type":"object"}, "notes": {"type":"string"}}}"#
    ),
    // -- ingestion --------------------------------------------------------
    route!(
        Post,
        Write,
        "ingestion.text",
        "/ingest",
        "Ingest raw text into the memory pipeline.",
        r#"{"type":"object","properties":{"text":{"type":"string"},"url":{"type":"string"},"title":{"type":"string"},"source":{"type":"string"},"entity_ids":{"type":"array","items":{"type":"integer"}},"project_ids":{"type":"array","items":{"type":"integer"}},"episode_id":{"type":"integer"}},"required":["text"]}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.bulk",
        "/import/bulk",
        "Bulk import of memories from a structured payload.",
        r#"{"type":"object","properties":{"text":{"type":"string"},"url":{"type":"string"},"format":{"type":"string"},"mode":{"type":"string"},"source":{"type":"string"},"category":{"type":"string"},"project_id":{"type":"integer"},"episode_id":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.json",
        "/import/json",
        "Import memories from a JSON dump.",
        r#"{"type":"object","properties":{"version":{"type":"string"},"memories":{"type":"array","items":{"type":"object"}}}}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.upload_init",
        "/ingest/upload/init",
        "Begin a chunked upload session.",
        r#"{"type":"object","properties":{"filename":{"type":"string"},"content_type":{"type":"string"},"source":{"type":"string"},"total_size":{"type":"integer"},"total_chunks":{"type":"integer"},"chunk_size":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.upload_chunk",
        "/ingest/upload/chunk",
        "Submit one chunk in a chunked upload.",
        r#"{"type":"object","properties":{"upload_id":{"type":"string"},"chunk_index":{"type":"integer"},"chunk_hash":{"type":"string","description":"hex-encoded SHA-256 of decoded chunk bytes"},"data":{"type":"string","description":"base64-encoded chunk"}},"required":["upload_id","chunk_index","chunk_hash","data"]}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.upload_complete",
        "/ingest/upload/complete",
        "Finalise a chunked upload.",
        r#"{"type":"object","properties":{"upload_id":{"type":"string"},"total_chunks":{"type":"integer"},"final_sha256":{"type":"string"},"mode":{"type":"string"},"format":{"type":"string"},"category":{"type":"string"},"project_id":{"type":"integer"},"episode_id":{"type":"integer"}},"required":["upload_id"]}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.upload_abort",
        "/ingest/upload/abort",
        "Abort a chunked upload.",
        r#"{"type": "object", "properties": {"upload_id": {"type":"string"}}, "required": ["upload_id"]}"#
    ),
    route!(
        Get,
        Read,
        "ingestion.upload_status",
        "/ingest/upload/{upload_id}/status",
        "Check status of a chunked upload.",
        r#"{"type": "object", "properties": {"upload_id": {"type":"string"}}, "required": ["upload_id"]}"#
    ),
    // -- inbox ------------------------------------------------------------
    route!(
        Get,
        Read,
        "inbox.list",
        "/inbox",
        "List pending inbox items.",
        r#"{"type": "object", "properties": {"limit": {"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "inbox.approve",
        "/inbox/{id}/approve",
        "Approve an inbox item.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    route!(
        Post,
        Write,
        "inbox.reject",
        "/inbox/{id}/reject",
        "Reject an inbox item.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}, "reason": {"type":"string"}}, "required": ["id"]}"#
    ),
    route!(
        Post,
        Write,
        "inbox.edit",
        "/inbox/{id}/edit",
        "Edit an inbox item before approving.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}, "patch": {"type":"object"}}, "required": ["id", "patch"]}"#
    ),
    route!(
        Post,
        Write,
        "inbox.bulk",
        "/inbox/bulk",
        "Apply an action to multiple inbox items at once.",
        r#"{"type":"object","properties":{"ids":{"type":"array","items":{"type":"integer"}},"action":{"type":"string"}},"required":["ids","action"]}"#
    ),
    // -- projects ---------------------------------------------------------
    route!(
        Get,
        Read,
        "projects.list",
        "/projects",
        "List projects.",
        ["memory_projects"],
        r#"{"type": "object"}"#
    ),
    route!(
        Post,
        Write,
        "projects.create",
        "/projects",
        "Create a project.",
        r#"{"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"}},"required":["name"]}"#
    ),
    route!(
        Get,
        Read,
        "projects.get",
        "/projects/{id}",
        "Fetch a project by id.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}}, "required": ["id"]}"#
    ),
    route!(
        Put,
        Write,
        "projects.update",
        "/projects/{id}",
        "Update a project.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"name":{"type":"string"},"description":{"type":"string"}},"required":["id"]}"#
    ),
    route!(
        Put,
        Write,
        "projects.link_memory",
        "/projects/{id}/memories/{mid}",
        "Link a memory to a project.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}, "mid": {"type":"integer"}}, "required": ["id", "mid"]}"#
    ),
    route!(
        Delete,
        Write,
        "projects.unlink_memory",
        "/projects/{id}/memories/{mid}",
        "Unlink a memory from a project.",
        r#"{"type": "object", "properties": {"id": {"type":"integer"}, "mid": {"type":"integer"}}, "required": ["id", "mid"]}"#
    ),
    // -- approvals --------------------------------------------------------
    route!(
        Post,
        Write,
        "approvals.create",
        "/approvals",
        "Submit an approval request.",
        r#"{"type":"object","properties":{"subject":{"type":"string"},"detail":{"type":"string"},"metadata":{"type":"object"}},"required":["subject"]}"#
    ),
    route!(
        Get,
        Read,
        "approvals.list_pending",
        "/approvals/pending",
        "List approvals awaiting a decision.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "approvals.get",
        "/approvals/{id}",
        "Fetch one approval request.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "approvals.decide",
        "/approvals/{id}/decide",
        "Approve or deny a pending approval.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"decision":{"type":"string","enum":["approve","deny"]},"reason":{"type":"string"}},"required":["id","decision"]}"#
    ),
    // -- audit ------------------------------------------------------------
    route!(
        Get,
        Read,
        "audit.list",
        "/audit",
        "Stream audit log entries.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"},"since":{"type":"string"}}}"#
    ),
    // -- policy -----------------------------------------------------------
    route!(
        Get,
        Read,
        "policy.mandatory",
        "/policy/mandatory",
        "Fetch the mandatory policy bundle.",
        r#"{"type":"object"}"#
    ),
    // -- gate -------------------------------------------------------------
    route!(
        Post,
        Read,
        "gate.check",
        "/gate/check",
        "Pre-flight a tool invocation against gate policy.",
        r#"{"type":"object","properties":{"command":{"type":"string"},"agent":{"type":"string"},"context":{"type":"string"},"tool_name":{"type":"string"},"session_id":{"type":"string"},"skip_approval":{"type":"boolean"}},"required":["command","agent"]}"#
    ),
    route!(
        Post,
        Write,
        "gate.respond",
        "/gate/respond",
        "Respond to a gate prompt.",
        r#"{"type":"object","properties":{"gate_id":{"type":"integer"},"approved":{"type":"boolean"},"reason":{"type":"string"}},"required":["gate_id","approved"]}"#
    ),
    route!(
        Post,
        Write,
        "gate.complete",
        "/gate/complete",
        "Mark a gate-tracked action complete.",
        r#"{"type":"object","properties":{"gate_id":{"type":"integer"},"output":{"type":"string"},"known_secrets":{"type":"array","items":{"type":"string"}}},"required":["gate_id","output"]}"#
    ),
    route!(
        Post,
        Write,
        "gate.complete_latest",
        "/gate/complete-latest",
        "Mark the most recent gate-tracked action complete.",
        r#"{"type":"object","properties":{"session_id":{"type":"string"},"output":{"type":"string"},"known_secrets":{"type":"array","items":{"type":"string"}}},"required":["session_id","output"]}"#
    ),
    route!(
        Post,
        Read,
        "gate.guard",
        "/guard",
        "Run content against the policy guard.",
        ["memory_guard"],
        r#"{"type":"object","properties":{"action":{"type":"string"}},"required":["action"]}"#
    ),
    // -- security ---------------------------------------------------------
    route!(
        Get,
        Read,
        "security.rate_limit",
        "/rate-limit/{key}",
        "Rate-limit status for a given key.",
        r#"{"type":"object","properties":{"key":{"type":"string"}},"required":["key"]}"#
    ),
    route!(
        Get,
        Read,
        "security.quota",
        "/quota",
        "Current quota usage.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Write,
        "security.record_usage",
        "/usage",
        "Record a usage event against quota.",
        r#"{"type":"object","properties":{"event":{"type":"string"},"qty":{"type":"integer"}},"required":["event"]}"#
    ),
    // -- portability ------------------------------------------------------
    route!(
        Get,
        Read,
        "portability.export",
        "/export",
        "Export user data for portability.",
        r#"{"type":"object","properties":{"format":{"type":"string"}}}"#
    ),
    route!(
        Post,
        Write,
        "portability.import",
        "/import",
        "Import a portable bundle.",
        r#"{"type":"object","properties":{"data":{"type":"object"}},"required":["data"]}"#
    ),
    // -- webhooks ---------------------------------------------------------
    route!(
        Post,
        Write,
        "webhooks.test",
        "/webhooks/test/{id}",
        "Fire a test event against a registered webhook.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    // -- auth_keys --------------------------------------------------------
    route!(
        Post,
        Admin,
        "auth_keys.create",
        "/keys",
        "Create an API key.",
        r#"{"type":"object","properties":{"name":{"type":"string"},"scopes":{"type":"array","items":{"type":"string"}}},"required":["name","scopes"]}"#
    ),
    route!(
        Get,
        Read,
        "auth_keys.list",
        "/keys",
        "List API keys.",
        r#"{"type":"object"}"#
    ),
    route!(
        Delete,
        Admin,
        "auth_keys.revoke",
        "/keys/{id}",
        "Revoke an API key.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Admin,
        "auth_keys.rotate",
        "/keys/rotate",
        "Rotate an API key.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Admin,
        "auth_keys.create_space",
        "/spaces",
        "Create a memory space.",
        r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#
    ),
    route!(
        Get,
        Read,
        "auth_keys.list_spaces",
        "/spaces",
        "List memory spaces.",
        r#"{"type":"object"}"#
    ),
    route!(
        Delete,
        Admin,
        "auth_keys.delete_space",
        "/spaces/{id}",
        "Delete a memory space.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    // -- identities -------------------------------------------------------
    route!(
        Get,
        Read,
        "identities.list",
        "/identities",
        "List enrolled identities.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "identities.audit",
        "/identities/{id}/audit",
        "Per-identity audit trail.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    // -- identity_keys ----------------------------------------------------
    route!(
        Post,
        Write,
        "identity_keys.enroll",
        "/identity-keys/enroll",
        "Enroll a new identity key (PIV/Ed25519).",
        r#"{"type":"object","properties":{"pubkey_pem":{"type":"string"},"label":{"type":"string"},"sig_hex":{"type":"string"}},"required":["pubkey_pem"]}"#
    ),
    route!(
        Get,
        Read,
        "identity_keys.list",
        "/identity-keys",
        "List all identity keys (admin view).",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "identity_keys.mine",
        "/identity-keys/mine",
        "List the caller's own identity keys.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Write,
        "identity_keys.revoke",
        "/identity-keys/{id}/revoke",
        "Revoke an identity key.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Admin,
        "identity_keys.invite",
        "/identity-keys/invite",
        "Issue an invite for identity-key enrollment.",
        r#"{"type":"object","properties":{"label":{"type":"string"},"ttl_seconds":{"type":"integer"}}}"#
    ),
    // -- agents -----------------------------------------------------------
    route!(
        Post,
        Write,
        "agents.register",
        "/agents",
        "Register a new agent.",
        r#"{"type":"object","properties":{"name":{"type":"string"},"agent_type":{"type":"string"},"description":{"type":"string"},"capabilities":{"type":"array","items":{"type":"string"}},"config":{"type":"object"}},"required":["name"]}"#
    ),
    route!(
        Get,
        Read,
        "agents.list",
        "/agents",
        "List registered agents.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "agents.get",
        "/agents/{id}",
        "Fetch an agent record.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Admin,
        "agents.revoke",
        "/agents/{id}/revoke",
        "Revoke an agent.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "agents.passport",
        "/agents/{id}/passport",
        "Fetch an agent's passport bundle.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "agents.link_key",
        "/agents/{id}/link-key",
        "Link an identity key to an agent.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"key_id":{"type":"integer"}},"required":["id","key_id"]}"#
    ),
    route!(
        Get,
        Read,
        "agents.executions",
        "/agents/{id}/executions",
        "List an agent's recorded executions.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"limit":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "agents.verify",
        "/verify",
        "Verify a code change against the spec (agent-forge verify).",
        r#"{"type":"object","properties":{"passport":{"type":"object"},"execution":{"type":"object"},"message":{"type":"object"},"tool_manifest":{"type":"object"}}}"#
    ),
    // -- conversations ----------------------------------------------------
    route!(
        Post,
        Write,
        "conversations.create",
        "/conversations",
        "Create a conversation thread.",
        r#"{"type":"object","properties":{"title":{"type":"string"},"metadata":{"type":"object"}}}"#
    ),
    route!(
        Get,
        Read,
        "conversations.list",
        "/conversations",
        "List conversation threads.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "conversations.add_message",
        "/conversations/{id}/messages",
        "Append a message to a conversation.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"role":{"type":"string"},"content":{"type":"string"}},"required":["id","content"]}"#
    ),
    route!(
        Get,
        Read,
        "conversations.list_messages",
        "/conversations/{id}/messages",
        "List messages in a conversation.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"limit":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Read,
        "conversations.search_messages",
        "/messages/search",
        "Search across conversation messages.",
        r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]}"#
    ),
    // -- errors -----------------------------------------------------------
    route!(
        Post,
        Write,
        "errors.report",
        "/errors",
        "Report an agent error event.",
        r#"{"type":"object","properties":{"agent":{"type":"string"},"message":{"type":"string"},"context":{"type":"object"}},"required":["message"]}"#
    ),
    route!(
        Get,
        Read,
        "errors.list",
        "/errors",
        "List recently reported errors.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    // -- supervisor -------------------------------------------------------
    route!(
        Post,
        Write,
        "supervisor.inject",
        "/supervisor/inject",
        "Inject a supervisor directive.",
        r#"{"type":"object","properties":{"session_id":{"type":"string"},"rule_id":{"type":"string"},"severity":{"type":"string"},"message":{"type":"string"}},"required":["session_id","rule_id","severity","message"]}"#
    ),
    route!(
        Get,
        Read,
        "supervisor.pending",
        "/supervisor/pending",
        "List pending supervisor directives.",
        r#"{"type":"object","properties":{"session_id":{"type":"string"}},"required":["session_id"]}"#
    ),
    // -- brain (Hopfield) -------------------------------------------------
    route!(
        Get,
        Read,
        "brain.stats",
        "/brain/stats",
        "Hopfield-network capacity / activation stats.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Read,
        "brain.query",
        "/brain/query",
        "Query the Hopfield brain with a partial cue.",
        r#"{"type":"object","properties":{"query":{"type":"string"},"top_k":{"type":"integer"},"beta":{"type":"number"},"spread_hops":{"type":"integer"}},"required":["query"]}"#
    ),
    route!(
        Post,
        Write,
        "brain.absorb",
        "/brain/absorb",
        "Absorb a memory into the Hopfield brain by ID.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "brain.dream",
        "/brain/dream",
        "Trigger an offline consolidation pass.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Write,
        "brain.feedback",
        "/brain/feedback",
        "Provide feedback on a Hopfield recall.",
        r#"{"type":"object","properties":{"memory_ids":{"type":"array","items":{"type":"integer"}},"edge_pairs":{"type":"array","items":{"type":"array","items":{"type":"integer"}}},"useful":{"type":"boolean"}},"required":["memory_ids","edge_pairs","useful"]}"#
    ),
    route!(
        Post,
        Admin,
        "brain.decay",
        "/brain/decay",
        "Apply a decay sweep to stored patterns.",
        r#"{"type":"object","properties":{"ticks":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Admin,
        "brain.evolution_train",
        "/brain/evolution/train",
        "Run the brain's evolutionary training cycle.",
        r#"{"type":"object","properties":{"iterations":{"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "brain.evolution_stats",
        "/brain/evolution/stats",
        "Brain evolutionary training stats.",
        r#"{"type":"object"}"#
    ),
    // -- graph (knowledge graph) -----------------------------------------
    route!(
        Get,
        Read,
        "graph.entity_memories",
        "/entities/{id}/memories",
        "List memories linked to an entity.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Read,
        "graph.entity_search",
        "/entities/{id}/search",
        "Search within the neighborhood of an entity.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"query":{"type":"string"}},"required":["id","query"]}"#
    ),
    route!(
        Post,
        Write,
        "graph.create_relationship",
        "/entity-relationships",
        "Create an entity-to-entity relationship.",
        r#"{"type":"object","properties":{"source_id":{"type":"integer"},"target_id":{"type":"integer"},"rel_type":{"type":"string"}},"required":["source_id","target_id","rel_type"]}"#
    ),
    route!(
        Get,
        Read,
        "graph.view",
        "/graph",
        "Default graph view.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "graph.raw",
        "/graph/raw",
        "Raw graph dump (nodes + edges).",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Admin,
        "graph.build",
        "/graph/build",
        "Rebuild the graph from current memories.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Read,
        "graph.search",
        "/graph/search",
        "Search the graph for entities / paths.",
        r#"{"type":"object","properties":{"query":{"type":"string"},"depth":{"type":"integer"}},"required":["query"]}"#
    ),
    // -- intelligence -----------------------------------------------------
    route!(
        Post,
        Write,
        "intelligence.consolidate",
        "/intelligence/consolidate",
        "Consolidate a set of memories into a composite.",
        r#"{"type":"object","properties":{"memory_ids":{"type":"array","items":{"type":"integer"}}},"required":["memory_ids"]}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.contradictions",
        "/contradictions/{memory_id}",
        "Find contradictions for a single memory.",
        r#"{"type":"object","properties":{"memory_id":{"type":"integer"}},"required":["memory_id"]}"#
    ),
    route!(
        Post,
        Read,
        "intelligence.scan_contradictions",
        "/contradictions",
        "Account-wide scan for contradictions.",
        ["intelligence.detect_contradictions"],
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.generate_digest",
        "/digests/generate",
        "Generate a memory digest.",
        r#"{"type":"object","properties":{"period":{"type":"string"}}}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.list_digests",
        "/intelligence/digests",
        "List previously generated digests.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.reflect",
        "/reflect",
        "Create a reflection record.",
        r#"{"type":"object","properties":{"content":{"type":"string"},"reflection_type":{"type":"string"},"source_memory_ids":{"type":"array","items":{"type":"integer"}},"confidence":{"type":"number"}},"required":["content","source_memory_ids"]}"#
    ),
    // -- admin ------------------------------------------------------------
    route!(
        Post,
        Admin,
        "admin.bootstrap",
        "/bootstrap",
        "Run the admin bootstrap routine.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Admin,
        "admin.stats",
        "/stats",
        "Server-wide stats.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Admin,
        "admin.get_settings",
        "/admin/settings",
        "Fetch server settings.",
        r#"{"type":"object"}"#
    ),
    route!(
        Put,
        Admin,
        "admin.put_settings",
        "/admin/settings",
        "Update server settings.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Admin,
        "admin.gc",
        "/admin/gc",
        "Run a garbage-collection sweep.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Admin,
        "admin.compact",
        "/admin/compact",
        "Compact (VACUUM) the database.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Admin,
        "admin.reembed",
        "/admin/reembed",
        "Re-embed memories with the current model.",
        r#"{"type":"object","properties":{"all":{"type":"boolean"}}}"#
    ),
    route!(
        Post,
        Admin,
        "admin.rebuild_fts",
        "/admin/rebuild-fts",
        "Rebuild the full-text-search index.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Admin,
        "admin.refresh_cache",
        "/admin/refresh-cache",
        "Refresh the in-memory caches.",
        r#"{"type":"object"}"#
    ),
    // -- skills (additional richer endpoints) -----------------------------
    route!(
        Post,
        Write,
        "skills.sync",
        "/skills/sync",
        "Bulk-sync skills from a source.",
        r#"{"type":"object","properties":{"items":{"type":"array"}},"required":["items"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.update",
        "/skills/{id}/update",
        "Update a skill record.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"patch":{"type":"object"}},"required":["id","patch"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.recompute",
        "/skills/{id}/recompute",
        "Recompute scores for a skill.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.record_execution",
        "/skills/{id}/execute",
        "Record a skill execution outcome.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"success":{"type":"boolean"},"notes":{"type":"string"}},"required":["id","success"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.list_executions",
        "/skills/{id}/executions",
        "List a skill's prior executions.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"limit":{"type":"integer"}},"required":["id"]}"#
    ),
    // -- context (8-layer assembly) ---------------------------------------
    route!(
        Post,
        Read,
        "context.build",
        "/context",
        "Build an 8-layer context bundle for a task.",
        ["context.assemble_context", "memory_context"],
        r#"{"type":"object","properties":{"query":{"type":"string"},"max_tokens":{"type":"integer"},"model_id":{"type":"string"},"source":{"type":"string"},"session":{"type":"string"}},"required":["query"]}"#
    ),
    route!(
        Post,
        Read,
        "context.build_stream",
        "/context/stream",
        "Streamed context-build endpoint.",
        r#"{"type":"object","properties":{"task":{"type":"string"}},"required":["task"]}"#
    ),
    // -- artifacts --------------------------------------------------------
    route!(
        Get,
        Read,
        "artifacts.stats",
        "/artifacts/stats",
        "Aggregate artifact-store statistics.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "artifacts.download",
        "/artifact/{id}",
        "Download an artifact by id.",
        r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#
    ),
    route!(
        Delete,
        Write,
        "artifacts.delete",
        "/artifact/{id}",
        "Delete an artifact by id.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Read,
        "artifacts.search",
        "/artifacts/search",
        "Full-text search across artifact name and content.",
        r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"},"memory_id":{"type":"integer"}},"required":["query"]}"#
    ),
    // -- axon (events) ----------------------------------------------------
    route!(
        Post,
        Write,
        "axon.publish",
        "/axon/publish",
        "Publish an event to Axon.",
        ["services.axon_publish"],
        r#"{"type":"object","properties":{"channel":{"type":"string"},"action":{"type":"string"},"payload":{"type":"object"},"source":{"type":"string"},"agent":{"type":"string"}},"required":["channel","action"]}"#
    ),
    route!(
        Get,
        Read,
        "axon.list_events",
        "/axon/events",
        "List Axon events.",
        ["services.axon_consume"],
        r#"{"type":"object","properties":{"channel":{"type":"string"},"limit":{"type":"integer"},"since":{"type":"string"}}}"#
    ),
    route!(
        Get,
        Read,
        "axon.get_event",
        "/axon/events/{id}",
        "Fetch one Axon event.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "axon.list_subscriptions",
        "/axon/subscriptions",
        "List Axon subscriptions.",
        r#"{"type":"object","properties":{"agent":{"type":"string"}},"required":["agent"]}"#
    ),
    route!(
        Post,
        Read,
        "axon.poll",
        "/axon/poll",
        "Long-poll an Axon channel for new events.",
        r#"{"type":"object","properties":{"channel":{"type":"string"},"cursor":{"type":"string"}},"required":["channel"]}"#
    ),
    route!(
        Get,
        Read,
        "axon.cursor",
        "/axon/cursor",
        "Get the current Axon cursor.",
        r#"{"type":"object","properties":{"agent":{"type":"string"},"channel":{"type":"string"}},"required":["agent","channel"]}"#
    ),
    route!(
        Get,
        Read,
        "axon.stats",
        "/axon/stats",
        "Axon channel statistics.",
        r#"{"type":"object"}"#
    ),
    // -- broca (actions) --------------------------------------------------
    route!(
        Get,
        Read,
        "broca.get_action",
        "/broca/actions/{id}",
        "Fetch one Broca action record.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "broca.feed",
        "/broca/feed",
        "Live feed of Broca-logged actions.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "broca.stats",
        "/broca/stats",
        "Broca action statistics.",
        r#"{"type":"object"}"#
    ),
    // -- soma (agent presence) --------------------------------------------
    route!(
        Post,
        Write,
        "soma.heartbeat",
        "/soma/agents/{id}/heartbeat",
        "Agent keepalive heartbeat.",
        ["services.soma_heartbeat"],
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "soma.list_logs",
        "/soma/agents/{id}/logs",
        "Recent Soma log entries for an agent.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"limit":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "soma.add_group_member",
        "/soma/groups/{id}/members",
        "Add an agent to a Soma group.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"agent_id":{"type":"integer"}},"required":["id","agent_id"]}"#
    ),
    route!(
        Get,
        Read,
        "soma.stats",
        "/soma/stats",
        "Soma presence statistics.",
        r#"{"type":"object"}"#
    ),
    // -- thymus (evaluation) ----------------------------------------------
    route!(
        Post,
        Write,
        "thymus.evaluate",
        "/thymus/evaluate",
        "Record an evaluation outcome.",
        ["services.thymus_review"],
        r#"{"type":"object","properties":{"rubric_id":{"type":"integer"},"agent":{"type":"string"},"evaluator":{"type":"string"},"subject":{"type":"string"},"scores":{"type":"object"},"input":{"type":"object"},"output":{"type":"object"},"notes":{"type":"string"}},"required":["rubric_id","agent","evaluator","subject","scores"]}"#
    ),
    route!(
        Get,
        Read,
        "thymus.list_evaluations",
        "/thymus/evaluations",
        "List evaluation records.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "thymus.get_evaluation",
        "/thymus/evaluations/{id}",
        "Fetch one evaluation record.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "thymus.metric_summary",
        "/thymus/metrics/summary",
        "Aggregate metric summary.",
        r#"{"type":"object","properties":{"window":{"type":"string"}}}"#
    ),
    route!(
        Get,
        Read,
        "thymus.stats",
        "/thymus/stats",
        "Thymus evaluation statistics.",
        r#"{"type":"object"}"#
    ),
    // -- growth -----------------------------------------------------------
    route!(
        Post,
        Write,
        "growth.reflect",
        "/growth/reflect",
        "Record a growth-reflection note.",
        r#"{"type":"object","properties":{"service":{"type":"string"},"context":{"type":"array","items":{"type":"string"}},"existing_growth":{"type":"string"},"prompt_override":{"type":"string"}},"required":["service"]}"#
    ),
    route!(
        Get,
        Read,
        "growth.observations",
        "/growth/observations",
        "List growth observations.",
        r#"{"type":"object","properties":{"limit":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "growth.materialize",
        "/growth/materialize",
        "Materialise growth notes into durable memory.",
        r#"{"type":"object","properties":{"observation_id":{"type":"integer"}},"required":["observation_id"]}"#
    ),
    // -- commerce ---------------------------------------------------------
    route!(
        Post,
        Write,
        "commerce.create_quote",
        "/commerce/quotes",
        "Create a commerce quote.",
        r#"{"type":"object","properties":{"item":{"type":"string"},"qty":{"type":"integer"}},"required":["item"]}"#
    ),
    route!(
        Get,
        Read,
        "commerce.get_quote",
        "/commerce/quotes/{id}",
        "Fetch a commerce quote.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Read,
        "commerce.budget_check",
        "/commerce/check",
        "Pre-flight a spend against budget.",
        r#"{"type":"object","properties":{"quote_id":{"type":"string"}},"required":["quote_id"]}"#
    ),
    route!(
        Get,
        Read,
        "commerce.reconciliation",
        "/commerce/reconciliation",
        "Fetch the latest reconciliation report.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "commerce.balance",
        "/commerce/balance",
        "Fetch the current balance.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "commerce.pricing",
        "/commerce/pricing",
        "List pricing tables.",
        r#"{"type":"object"}"#
    ),
    // -- loom (workflows) -------------------------------------------------
    route!(
        Get,
        Read,
        "loom.get_run",
        "/loom/runs/{id}",
        "Fetch one Loom run.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "loom.cancel_run",
        "/loom/runs/{id}/cancel",
        "Cancel a running Loom workflow.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "loom.get_steps",
        "/loom/runs/{id}/steps",
        "List a run's steps.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "loom.get_logs",
        "/loom/runs/{id}/logs",
        "Fetch the log lines for a run.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"limit":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "loom.complete_step",
        "/loom/steps/{id}/complete",
        "Mark a step complete.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"output":{"type":"object"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "loom.fail_step",
        "/loom/steps/{id}/fail",
        "Mark a step failed.",
        r#"{"type":"object","properties":{"id":{"type":"integer"},"error":{"type":"string"}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "loom.stats",
        "/loom/stats",
        "Loom workflow statistics.",
        r#"{"type":"object"}"#
    ),
    // -- pack -------------------------------------------------------------
    route!(
        Post,
        Read,
        "pack.memories",
        "/pack",
        "Pack a memory set into a portable bundle.",
        r#"{"type":"object","properties":{"memory_ids":{"type":"array","items":{"type":"integer"}},"format":{"type":"string"}},"required":["memory_ids"]}"#
    ),
    // -- onboard ----------------------------------------------------------
    route!(
        Post,
        Write,
        "onboard.run",
        "/onboard",
        "Run the onboarding routine.",
        r#"{"type":"object","properties":{"profile":{"type":"object"}}}"#
    ),
    route!(
        Post,
        Write,
        "onboard.fetch_url",
        "/fetch",
        "Fetch a URL during onboarding.",
        r#"{"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}"#
    ),
    // -- grounding --------------------------------------------------------
    route!(
        Get,
        Read,
        "grounding.list_tools",
        "/grounding/tools",
        "List grounding tools.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Write,
        "grounding.execute",
        "/grounding/execute",
        "Execute a grounding tool.",
        r#"{"type":"object","properties":{"tool":{"type":"string"},"args":{"type":"object"}},"required":["tool"]}"#
    ),
    route!(
        Get,
        Read,
        "grounding.quality",
        "/grounding/quality",
        "Grounding quality metrics.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "grounding.list_providers",
        "/grounding/providers",
        "List available grounding providers.",
        r#"{"type":"object"}"#
    ),
    // -- batch ------------------------------------------------------------
    route!(
        Post,
        Write,
        "batch.run",
        "/batch",
        "Run a batch of inline requests in one call.",
        r#"{"type":"object","properties":{"requests":{"type":"array"}},"required":["requests"]}"#
    ),
    // -- dispatch ---------------------------------------------------------
    route!(
        Get,
        Read,
        "dispatch.list_configs",
        "/dispatch/configs",
        "List dispatch configurations.",
        r#"{"type":"object"}"#
    ),
    route!(
        Post,
        Write,
        "dispatch.create_config",
        "/dispatch/configs",
        "Create a dispatch configuration.",
        r#"{"type":"object","properties":{"name":{"type":"string"},"config":{"type":"object"}},"required":["name","config"]}"#
    ),
    // -- mcp_schema -------------------------------------------------------
    route!(
        Get,
        Read,
        "mcp_schema.get",
        "/mcp/schema",
        "Fetch the MCP-flavoured schema descriptor.",
        r#"{"type":"object"}"#
    ),
    // -- schema (discovery) -----------------------------------------------
    route!(
        Get,
        Read,
        "schema.index",
        "/schema",
        "Top-level schema index.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "schema.memory",
        "/schema/memory",
        "Memory schema.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "schema.services",
        "/schema/services",
        "Coordination services schema.",
        r#"{"type":"object"}"#
    ),
    route!(
        Get,
        Read,
        "schema.graph",
        "/schema/graph",
        "Graph schema.",
        r#"{"type":"object"}"#
    ),
    // -- users ------------------------------------------------------------
    route!(
        Post,
        Admin,
        "users.create",
        "/users",
        "Create a user.",
        r#"{"type":"object","properties":{"username":{"type":"string"},"email":{"type":"string"},"role":{"type":"string"}},"required":["username"]}"#
    ),
    route!(
        Get,
        Admin,
        "users.list",
        "/users",
        "List users.",
        r#"{"type":"object"}"#
    ),
    route!(
        Delete,
        Admin,
        "users.deactivate",
        "/users/{id}",
        "Deactivate a user account.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    // -- forge (agent-forge stateful operations) ----------------------------------
    route!(
        Post,
        Write,
        "forge.spec_task",
        "/forge/spec-task",
        "Create a new task spec with acceptance criteria and file coverage for gate enforcement.",
        ["forge_spec_task"],
        r#"{"type":"object","properties":{"session_id":{"type":"string"},"task_description":{"type":"string"},"task_type":{"type":"string"},"acceptance_criteria":{"type":"array","items":{"type":"string"}},"interface_contract":{"type":"string"},"edge_cases":{"type":"array","items":{"type":"string"}},"files_to_touch":{"type":"array","items":{"type":"string"}},"dependencies":{"type":"string"}},"required":["task_description","task_type","acceptance_criteria","interface_contract","edge_cases"]}"#
    ),
    route!(
        Post,
        Write,
        "forge.update_spec",
        "/forge/update-spec",
        "Transition a spec to a new lifecycle status (active, completed, failed, blocked).",
        ["forge_update_spec"],
        r#"{"type":"object","properties":{"spec_id":{"type":"string"},"status":{"type":"string"},"note":{"type":"string"}},"required":["spec_id","status"]}"#
    ),
    route!(
        Get,
        Read,
        "forge.list_specs",
        "/forge/specs",
        "List forge specs for the authenticated user, optionally filtered by status.",
        ["forge_list_specs"],
        r#"{"type":"object","properties":{"status":{"type":"string"},"limit":{"type":"integer"}}}"#
    ),
    route!(
        Get,
        Read,
        "forge.get_spec",
        "/forge/spec/{id}",
        "Fetch one full spec by ID including related sub-records.",
        ["forge_get_spec"],
        r#"{"type":"object","properties":{"id":{"type":"string"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "forge.log_hypothesis",
        "/forge/log-hypothesis",
        "Record a new hypothesis before touching code in response to a bug.",
        ["forge_log_hypothesis"],
        r#"{"type":"object","properties":{"session_id":{"type":"string"},"bug_description":{"type":"string"},"hypothesis":{"type":"string"},"confidence":{"type":"number"},"spec_id":{"type":"string"}},"required":["bug_description","hypothesis"]}"#
    ),
    route!(
        Post,
        Write,
        "forge.log_outcome",
        "/forge/log-outcome",
        "Record the outcome of an existing hypothesis (correct, incorrect, or partial).",
        ["forge_log_outcome"],
        r#"{"type":"object","properties":{"hypothesis_id":{"type":"string"},"outcome":{"type":"string"},"notes":{"type":"string"}},"required":["hypothesis_id","outcome"]}"#
    ),
    route!(
        Get,
        Read,
        "forge.recall_errors",
        "/forge/recall-errors",
        "Search past hypotheses by keyword across bug description and hypothesis text.",
        ["forge_recall_errors"],
        r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}}}"#
    ),
    route!(
        Post,
        Write,
        "forge.consider_approaches",
        "/forge/consider-approaches",
        "Store two or more named design alternatives and return a structured comparison prompt.",
        ["forge_consider_approaches"],
        r#"{"type":"object","properties":{"spec_id":{"type":"string"},"problem":{"type":"string"},"approaches":{"type":"array","items":{"type":"object"}},"chosen_index":{"type":"integer"}},"required":["problem","approaches"]}"#
    ),
    route!(
        Post,
        Write,
        "forge.verify",
        "/forge/verify",
        "Record the result of a client-side verification run against a spec criterion.",
        ["forge_verify"],
        r#"{"type":"object","properties":{"spec_id":{"type":"string"},"command":{"type":"string"},"exit_code":{"type":"integer"},"success":{"type":"boolean"},"duration_ms":{"type":"integer"},"criteria_index":{"type":"integer"},"stdout":{"type":"string"},"stderr":{"type":"string"}},"required":["command","exit_code","success"]}"#
    ),
    route!(
        Post,
        Write,
        "forge.session_learn",
        "/forge/session-learn",
        "Persist a mid-session discovery to forge_session_learns.",
        ["forge_session_learn"],
        r#"{"type":"object","properties":{"discovery":{"type":"string"},"context":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"spec_id":{"type":"string"}},"required":["discovery"]}"#
    ),
    route!(
        Get,
        Read,
        "forge.session_recall",
        "/forge/session-recall",
        "Search forge_session_learns by keyword in the discovery text.",
        ["forge_session_recall"],
        r#"{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}}}"#
    ),
    // -- forge compute (stateless, backed by agent_forge library) ---------
    route!(
        Post,
        Read,
        "forge.think",
        "/forge/think",
        "Pure structured-reasoning prompt builder. Accepts a problem statement, optional constraints, and optional context.",
        ["forge_think"],
        r#"{"type":"object","properties":{"problem":{"type":"string"},"constraints":{"type":"array","items":{"type":"string"}},"context":{"type":"string"}}}"#
    ),
    route!(
        Post,
        Read,
        "forge.declare_unknowns",
        "/forge/declare-unknowns",
        "Partition unknowns into blocking and non-blocking sets and return a clear action directive.",
        ["forge_declare_unknowns"],
        r#"{"type":"object","properties":{"unknowns":{"type":"array","items":{"type":"object","properties":{"description":{"type":"string"},"blocking":{"type":"boolean"},"resolution_hint":{"type":"string"}},"required":["description","blocking"]}}}}"#
    ),
    route!(
        Post,
        Read,
        "forge.comment_check",
        "/forge/comment-check",
        "Scan a source file for declarations that lack a preceding comment and return a coverage report.",
        ["forge_comment_check"],
        r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"extension":{"type":"string"}}}"#
    ),
    route!(
        Post,
        Read,
        "forge.challenge_code",
        "/forge/challenge-code",
        "Build an adversarial review prompt for a source file, embedding a mechanical comment-coverage report.",
        ["forge_challenge_code"],
        r#"{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"},"extension":{"type":"string"}}}"#
    ),
    route!(
        Post,
        Read,
        "forge.repo_map",
        "/forge/repo-map",
        "Walk a directory tree, extract named symbols, and return a ranked symbol map within a configurable token budget.",
        ["forge_repo_map"],
        r#"{"type":"object","properties":{"path":{"type":"string"},"focus":{"type":"array","items":{"type":"string"}},"max_tokens":{"type":"integer"}},"required":["path"]}"#
    ),
    route!(
        Post,
        Read,
        "forge.search_code",
        "/forge/search-code",
        "Walk a directory tree and return symbols whose names contain the supplied query string (case-insensitive).",
        ["forge_search_code"],
        r#"{"type":"object","properties":{"query":{"type":"string"},"path":{"type":"string"},"symbol_type":{"type":"string"},"limit":{"type":"integer"}},"required":["path"]}"#
    ),
    // ===== auto-generated long-tail entries (mechanical, refine schemas/descriptions over time) =====
    // -- admin (generated) --
    route!(
        Post,
        Admin,
        "admin.backfill_facts",
        "/admin/backfill-facts",
        "Auto: POST /admin/backfill-facts.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.backfill_chunks",
        "/admin/backfill_chunks",
        "Auto: POST /admin/backfill_chunks.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.reapply_instincts",
        "/admin/brain/instincts/reapply",
        "Auto: POST /admin/brain/instincts/reapply.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.cold_storage",
        "/admin/cold-storage",
        "Auto: GET /admin/cold-storage.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.cred_proxy",
        "/admin/cred/proxy",
        "Auto: POST /admin/cred/proxy.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.cred_resolve",
        "/admin/cred/resolve",
        "Auto: POST /admin/cred/resolve.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.detect_communities",
        "/admin/detect-communities",
        "Auto: POST /admin/detect-communities.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.embedding_info",
        "/admin/embedding-info",
        "Auto: GET /admin/embedding-info.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.export",
        "/admin/export",
        "Auto: GET /admin/export.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.get_maintenance",
        "/admin/maintenance",
        "Auto: GET /admin/maintenance.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.post_maintenance",
        "/admin/maintenance",
        "Auto: POST /admin/maintenance.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.migration_status",
        "/admin/migrations",
        "Auto: GET /admin/migrations.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.migrate_down",
        "/admin/migrations/down",
        "Auto: POST /admin/migrations/down.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.monolith_drain",
        "/admin/monolith/drain",
        "Auto: POST /admin/monolith/drain.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.monolith_status",
        "/admin/monolith/status",
        "Auto: GET /admin/monolith/status.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.pagerank_rebuild",
        "/admin/pagerank/rebuild",
        "Auto: POST /admin/pagerank/rebuild.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.pitr_prepare",
        "/admin/pitr/prepare-restore",
        "Auto: POST /admin/pitr/prepare-restore.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.pitr_snapshots",
        "/admin/pitr/snapshots",
        "Auto: GET /admin/pitr/snapshots.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.providers",
        "/admin/providers",
        "Auto: GET /admin/providers.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.get_quotas",
        "/admin/quotas",
        "Auto: GET /admin/quotas.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Put,
        Admin,
        "admin.put_quotas",
        "/admin/quotas",
        "Auto: PUT /admin/quotas.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.rebuild_cooccurrences",
        "/admin/rebuild-cooccurrences",
        "Auto: POST /admin/rebuild-cooccurrences.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.reset_user",
        "/admin/reset",
        "Auto: POST /admin/reset.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.post_safe_mode_exit",
        "/admin/safe-mode/exit",
        "Auto: POST /admin/safe-mode/exit.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.scale_report",
        "/admin/scale-report",
        "Auto: GET /admin/scale-report.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.schema",
        "/admin/schema",
        "Auto: GET /admin/schema.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.sla",
        "/admin/sla",
        "Auto: GET /admin/sla.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.sla_reset",
        "/admin/sla/reset",
        "Auto: POST /admin/sla/reset.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.tasks",
        "/admin/tasks",
        "Auto: GET /admin/tasks.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.tenants",
        "/admin/tenants",
        "Auto: GET /admin/tenants.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.usage",
        "/admin/usage",
        "Auto: GET /admin/usage.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.vector_sync_replay",
        "/admin/vector-sync/replay",
        "Auto: POST /admin/vector-sync/replay.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.vector_rebuild_index",
        "/admin/vector/rebuild-index",
        "Auto: POST /admin/vector/rebuild-index.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.vector_chunk_sync",
        "/admin/vector/chunk-sync",
        "Auto: POST /admin/vector/chunk-sync.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.vector_health",
        "/admin/vector_health",
        "Auto: GET /admin/vector_health.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Admin,
        "admin.backup",
        "/backup",
        "Auto: GET /backup.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.backup_verify",
        "/backup/verify",
        "Auto: POST /backup/verify.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.checkpoint",
        "/checkpoint",
        "Auto: POST /checkpoint.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.deprovision_tenant",
        "/tenants/deprovision",
        "Auto: POST /tenants/deprovision.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Admin,
        "admin.provision_tenant",
        "/tenants/provision",
        "Auto: POST /tenants/provision.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- artifacts (generated) --
    route!(
        Get,
        Read,
        "artifacts.list_for_memory",
        "/artifacts/{memory_id}",
        "Auto: GET /artifacts/{memory_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"memory_id":{}},"required":["memory_id"]}"#
    ),
    route!(
        Post,
        Write,
        "artifacts.upload_artifact",
        "/artifacts/{memory_id}",
        "Auto: POST /artifacts/{memory_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"memory_id":{}},"required":["memory_id"]}"#
    ),
    // -- axon (generated) --
    route!(
        Get,
        Read,
        "axon.list_channels",
        "/axon/channels",
        "Auto: GET /axon/channels.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "axon.create_channel",
        "/axon/channels",
        "Auto: POST /axon/channels.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "axon.unsubscribe",
        "/axon/subscribe",
        "Auto: DELETE /axon/subscribe.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "axon.subscribe",
        "/axon/subscribe",
        "Auto: POST /axon/subscribe.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- brain (generated) --
    route!(
        Post,
        Write,
        "brain.evolution_feedback",
        "/brain/evolution/feedback",
        "Submit evolution feedback to the brain.",
        r#"{"type":"object","properties":{"memory_ids":{"type":"array","items":{"type":"integer"}},"edge_pairs":{"type":"array","items":{"type":"array"}},"useful":{"type":"boolean"}},"required":["memory_ids","edge_pairs","useful"]}"#
    ),
    // -- broca (generated) --
    route!(
        Get,
        Read,
        "broca.list_actions",
        "/broca/actions",
        "Auto: GET /broca/actions.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "broca.log_action",
        "/broca/actions",
        "Log an action via Broca.",
        ["services.broca_log"],
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- conversations (generated) --
    route!(
        Post,
        Write,
        "conversations.bulk_insert",
        "/conversations/bulk",
        "Auto: POST /conversations/bulk.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "conversations.upsert",
        "/conversations/upsert",
        "Auto: POST /conversations/upsert.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "conversations.remove",
        "/conversations/{id}",
        "Auto: DELETE /conversations/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "conversations.get_one",
        "/conversations/{id}",
        "Auto: GET /conversations/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Patch,
        Write,
        "conversations.update",
        "/conversations/{id}",
        "Auto: PATCH /conversations/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- dispatch (generated) --
    route!(
        Delete,
        Write,
        "dispatch.delete_config",
        "/dispatch/configs/{skill_name}",
        "Auto: DELETE /dispatch/configs/{skill_name}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"skill_name":{}},"required":["skill_name"]}"#
    ),
    route!(
        Get,
        Read,
        "dispatch.get_config",
        "/dispatch/configs/{skill_name}",
        "Auto: GET /dispatch/configs/{skill_name}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"skill_name":{}},"required":["skill_name"]}"#
    ),
    route!(
        Put,
        Write,
        "dispatch.update_config",
        "/dispatch/configs/{skill_name}",
        "Auto: PUT /dispatch/configs/{skill_name}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"skill_name":{}},"required":["skill_name"]}"#
    ),
    // -- docs (generated) --
    route!(
        Get,
        Read,
        "docs.swagger_ui",
        "/docs/",
        "Auto: GET /docs/.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "docs.openapi",
        "/docs/openapi.json",
        "Auto: GET /docs/openapi.json.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- graph (generated) --
    route!(
        Get,
        Read,
        "graph.communities",
        "/communities",
        "List Louvain communities over the entity graph.",
        ["graph.louvain_communities"],
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "graph.community_detail",
        "/communities/{id}",
        "Auto: GET /communities/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "graph.list_entities",
        "/entities",
        "Auto: GET /entities.",
        ["memory_entities"],
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "graph.create_entity",
        "/entities",
        "Auto: POST /entities.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "graph.delete_entity",
        "/entities/{id}",
        "Auto: DELETE /entities/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "graph.get_entity",
        "/entities/{id}",
        "Auto: GET /entities/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Put,
        Write,
        "graph.update_entity",
        "/entities/{id}",
        "Auto: PUT /entities/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "graph.entity_cooccurrences",
        "/entities/{id}/cooccurrences",
        "Auto: GET /entities/{id}/cooccurrences.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Delete,
        Write,
        "graph.unlink_entity_memory",
        "/entities/{id}/memories/{mid}",
        "Auto: DELETE /entities/{id}/memories/{mid}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{},"mid":{}},"required":["id","mid"]}"#
    ),
    route!(
        Put,
        Write,
        "graph.link_entity_memory",
        "/entities/{id}/memories/{mid}",
        "Auto: PUT /entities/{id}/memories/{mid}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{},"mid":{}},"required":["id","mid"]}"#
    ),
    route!(
        Delete,
        Write,
        "graph.delete_relationship",
        "/entities/{id}/relationships",
        "Auto: DELETE /entities/{id}/relationships.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "graph.entity_relationships",
        "/entities/{id}/relationships",
        "Auto: GET /entities/{id}/relationships.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "graph.facts",
        "/facts",
        "Auto: GET /facts.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "graph.detect_communities",
        "/graph/communities",
        "Auto: POST /graph/communities.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "graph.community_stats",
        "/graph/communities/stats",
        "Auto: GET /graph/communities/stats.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "graph.community_members",
        "/graph/communities/{id}/members",
        "Auto: GET /graph/communities/{id}/members.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "graph.rebuild_cooccurrences",
        "/graph/cooccurrences/rebuild",
        "Auto: POST /graph/cooccurrences/rebuild.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "graph.neighborhood",
        "/graph/neighborhood/{id}",
        "Fetch the neighborhood subgraph around an entity.",
        ["graph.get_neighbors"],
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "graph.pagerank",
        "/graph/pagerank",
        "Run PageRank over the entity graph.",
        ["graph.pagerank_top"],
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "graph.memory_entities",
        "/memory/{id}/entities",
        "Auto: GET /memory/{id}/entities.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- grounding (generated) --
    route!(
        Get,
        Read,
        "grounding.list_sessions",
        "/grounding/sessions",
        "Auto: GET /grounding/sessions.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "grounding.create_session",
        "/grounding/sessions",
        "Auto: POST /grounding/sessions.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "grounding.destroy_session",
        "/grounding/sessions/{id}",
        "Auto: DELETE /grounding/sessions/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "grounding.get_session",
        "/grounding/sessions/{id}",
        "Auto: GET /grounding/sessions/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- gui (generated) --
    route!(
        Get,
        Read,
        "gui.serve_login_css",
        "/_app/login.css",
        "Auto: GET /_app/login.css.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "gui.serve_login_js",
        "/_app/login.js",
        "Auto: GET /_app/login.js.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "gui.auth",
        "/gui/auth",
        "Auto: POST /gui/auth.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "gui.logout",
        "/gui/logout",
        "Auto: POST /gui/logout.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "gui.create_memory",
        "/gui/memories",
        "Auto: POST /gui/memories.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "gui.bulk_archive",
        "/gui/memories/bulk-archive",
        "Auto: POST /gui/memories/bulk-archive.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "gui.delete_memory",
        "/gui/memories/{id}",
        "Auto: DELETE /gui/memories/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Patch,
        Write,
        "gui.update_memory",
        "/gui/memories/{id}",
        "Auto: PATCH /gui/memories/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- health (generated) --
    route!(
        Get,
        Read,
        "health.get_live",
        "/health/live",
        "Auto: GET /health/live.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "health.get_ready",
        "/health/ready",
        "Auto: GET /health/ready.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "health.get_metrics",
        "/metrics",
        "Auto: GET /metrics.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- inbox (generated) --
    route!(
        Get,
        Read,
        "inbox.list_pending_legacy",
        "/pending",
        "Auto: GET /pending.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- ingestion (generated) --
    route!(
        Post,
        Write,
        "ingestion.import_mem0",
        "/import/mem0",
        "Auto: POST /import/mem0.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.import_supermemory",
        "/import/supermemory",
        "Auto: POST /import/supermemory.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "ingestion.ingest_text_stream",
        "/ingest/stream",
        "Auto: POST /ingest/stream.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- intelligence (generated) --
    route!(
        Post,
        Write,
        "intelligence.correct",
        "/correct",
        "Correct a memory's content.",
        r#"{"type":"object","properties":{"memory_id":{"type":"integer"},"content":{"type":"string"},"reason":{"type":"string"}},"required":["memory_id","content"]}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.deduplicate",
        "/deduplicate",
        "Auto: POST /deduplicate.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.duplicates",
        "/duplicates",
        "Auto: GET /duplicates.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.feedback",
        "/feedback",
        "Record user feedback on a memory.",
        r#"{"type":"object","properties":{"memory_id":{"type":"integer"},"rating":{"type":"string"},"context":{"type":"string"}},"required":["memory_id","rating"]}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.feedback_stats",
        "/feedback/stats",
        "Auto: GET /feedback/stats.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.causal_backward",
        "/intelligence/causal/backward/{memory_id}",
        "Auto: POST /intelligence/causal/backward/{memory_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"memory_id":{}},"required":["memory_id"]}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.list_chains",
        "/intelligence/causal/chains",
        "Auto: GET /intelligence/causal/chains.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.create_chain",
        "/intelligence/causal/chains",
        "Create a causal chain.",
        r#"{"type":"object","properties":{"root_memory_id":{"type":"integer"},"description":{"type":"string"}}}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.get_chain",
        "/intelligence/causal/chains/{id}",
        "Auto: GET /intelligence/causal/chains/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.add_link",
        "/intelligence/causal/links",
        "Add a causal link to a chain.",
        r#"{"type":"object","properties":{"chain_id":{"type":"integer"},"cause_memory_id":{"type":"integer"},"effect_memory_id":{"type":"integer"},"strength":{"type":"number"},"order_index":{"type":"integer"}},"required":["chain_id","cause_memory_id","effect_memory_id"]}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.candidates",
        "/intelligence/consolidation-candidates",
        "Auto: POST /intelligence/consolidation-candidates.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.list_consolidations",
        "/intelligence/consolidations",
        "Auto: GET /intelligence/consolidations.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.decompose",
        "/intelligence/decompose/{memory_id}",
        "Auto: POST /intelligence/decompose/{memory_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"memory_id":{}},"required":["memory_id"]}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.dream",
        "/intelligence/dream",
        "Auto: POST /intelligence/dream.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.dreamer_stats",
        "/intelligence/dreamer",
        "Auto: GET /intelligence/dreamer.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.extract",
        "/intelligence/extract",
        "Auto: POST /intelligence/extract.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.predictive_patterns",
        "/intelligence/predictive/patterns",
        "Auto: GET /intelligence/predictive/patterns.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.predictive_recall",
        "/intelligence/predictive/recall",
        "Auto: POST /intelligence/predictive/recall.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.predictive_sequences",
        "/intelligence/predictive/sequences",
        "Auto: POST /intelligence/predictive/sequences.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.reconsolidate",
        "/intelligence/reconsolidate/{memory_id}",
        "Auto: POST /intelligence/reconsolidate/{memory_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"memory_id":{}},"required":["memory_id"]}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.reconsolidation_candidates",
        "/intelligence/reconsolidation/candidates",
        "Auto: GET /intelligence/reconsolidation/candidates.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.list_reflections",
        "/intelligence/reflections",
        "Auto: GET /intelligence/reflections.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.create_reflection",
        "/intelligence/reflections",
        "Create an intelligence reflection.",
        r#"{"type":"object","properties":{"content":{"type":"string"},"reflection_type":{"type":"string"},"source_memory_ids":{"type":"array","items":{"type":"integer"}},"confidence":{"type":"number"}},"required":["content","source_memory_ids"]}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.generate_reflections",
        "/intelligence/reflections/generate",
        "Auto: POST /intelligence/reflections/generate.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.run_pipeline",
        "/intelligence/run",
        "Auto: POST /intelligence/run.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.sentiment_analyze",
        "/intelligence/sentiment/analyze",
        "Run sentiment analysis over memory content.",
        ["intelligence.sentiment"],
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.sentiment_history",
        "/intelligence/sentiment/history",
        "Auto: GET /intelligence/sentiment/history.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.detect_temporal",
        "/intelligence/temporal/detect",
        "Auto: POST /intelligence/temporal/detect.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.list_temporal",
        "/intelligence/temporal/patterns",
        "Auto: GET /intelligence/temporal/patterns.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.valence_profile",
        "/intelligence/valence/profile",
        "Auto: GET /intelligence/valence/profile.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.valence_score",
        "/intelligence/valence/score",
        "Auto: POST /intelligence/valence/score.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.valence_get",
        "/intelligence/valence/{memory_id}",
        "Auto: GET /intelligence/valence/{memory_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"memory_id":{}},"required":["memory_id"]}"#
    ),
    route!(
        Get,
        Read,
        "intelligence.memory_health",
        "/memory-health",
        "Auto: GET /memory-health.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.sweep",
        "/sweep",
        "Auto: POST /sweep.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "intelligence.time_travel",
        "/timetravel",
        "Query memories from a specific point in time.",
        r#"{"type":"object","properties":{"query":{"type":"string"},"timestamp":{"type":"string"},"limit":{"type":"integer"}},"required":["timestamp"]}"#
    ),
    // -- loom (generated) --
    route!(
        Get,
        Read,
        "loom.list_runs",
        "/loom/runs",
        "Auto: GET /loom/runs.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "loom.create_run",
        "/loom/runs",
        "Auto: POST /loom/runs.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "loom.list_workflows",
        "/loom/workflows",
        "Auto: GET /loom/workflows.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "loom.create_workflow",
        "/loom/workflows",
        "Auto: POST /loom/workflows.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "loom.delete_workflow",
        "/loom/workflows/{id}",
        "Auto: DELETE /loom/workflows/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "loom.get_workflow",
        "/loom/workflows/{id}",
        "Auto: GET /loom/workflows/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Patch,
        Write,
        "loom.update_workflow",
        "/loom/workflows/{id}",
        "Auto: PATCH /loom/workflows/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- memory (generated) --
    route!(
        Get,
        Read,
        "memory.get_links",
        "/links/{id}",
        "Auto: GET /links/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "memory.user_stats",
        "/me/stats",
        "Auto: GET /me/stats.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "memory.store_memory",
        "/memories",
        "Auto: POST /memories.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "memory.search_memories",
        "/memories/search",
        "Auto: POST /memories/search.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "memory.list_trashed",
        "/memory/trash",
        "Auto: GET /memory/trash.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "memory.restore_memory",
        "/memory/{id}/restore",
        "Auto: POST /memory/{id}/restore.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Put,
        Write,
        "memory.update_tags",
        "/memory/{id}/tags",
        "Auto: PUT /memory/{id}/tags.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "memory.profile",
        "/profile",
        "Auto: GET /profile.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "memory.synthesize_profile",
        "/profile/synthesize",
        "Auto: POST /profile/synthesize.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "memory.explain_search",
        "/search/explain",
        "Auto: POST /search/explain.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "memory.faceted_search",
        "/search/faceted",
        "Auto: POST /search/faceted.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "memory.list_tags",
        "/tags",
        "Auto: GET /tags.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "memory.search_tags",
        "/tags/search",
        "Auto: POST /tags/search.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "memory.version_chain",
        "/versions/{id}",
        "Auto: GET /versions/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- personality (generated) --
    route!(
        Get,
        Read,
        "personality.list_signals",
        "/personality/signals",
        "Auto: GET /personality/signals.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "personality.store_signal",
        "/personality/signals",
        "Auto: POST /personality/signals.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- platform (generated) --
    route!(
        Get,
        Read,
        "platform.get_sync_changes",
        "/sync/changes",
        "Auto: GET /sync/changes.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "platform.sync_receive",
        "/sync/receive",
        "Auto: POST /sync/receive.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- portability (generated) --
    route!(
        Delete,
        Write,
        "portability.delete_all_preferences",
        "/preferences",
        "Auto: DELETE /preferences.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "portability.list_preferences",
        "/preferences",
        "Auto: GET /preferences.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Put,
        Write,
        "portability.put_preferences",
        "/preferences",
        "Auto: PUT /preferences.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "portability.delete_preference",
        "/preferences/{key}",
        "Auto: DELETE /preferences/{key}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"key":{}},"required":["key"]}"#
    ),
    route!(
        Get,
        Read,
        "portability.get_preference",
        "/preferences/{key}",
        "Auto: GET /preferences/{key}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"key":{}},"required":["key"]}"#
    ),
    route!(
        Delete,
        Write,
        "portability.delete_state",
        "/state",
        "Auto: DELETE /state.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "portability.get_state",
        "/state",
        "Get portable app state, optionally filtered by key.",
        r#"{"type":"object","properties":{"key":{"type":"string"}}}"#
    ),
    // -- projects (generated) --
    route!(
        Delete,
        Write,
        "projects.delete_project",
        "/projects/{id}",
        "Auto: DELETE /projects/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- search (generated) --
    route!(
        Post,
        Write,
        "search.refresh_decay",
        "/decay/refresh",
        "Auto: POST /decay/refresh.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "search.get_decay_scores",
        "/decay/scores",
        "Get memory decay scores, optionally for a specific memory.",
        r#"{"type":"object","properties":{"memory_id":{"type":"integer"},"limit":{"type":"integer"},"order":{"type":"string"}}}"#
    ),
    route!(
        Post,
        Write,
        "search.web",
        "/search/web",
        "Auto: POST /search/web.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    // -- security (generated) --
    route!(
        Get,
        Read,
        "security.list_api_keys",
        "/api-keys",
        "Auto: GET /api-keys.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "security.create_api_key",
        "/api-keys",
        "Auto: POST /api-keys.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Admin,
        "security.delete_api_key",
        "/api-keys/{id}",
        "Auto: DELETE /api-keys/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- sessions (generated) --
    route!(
        Get,
        Read,
        "sessions.list_sessions",
        "/sessions",
        "Auto: GET /sessions.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "sessions.create_session",
        "/sessions",
        "Auto: POST /sessions.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "sessions.stream",
        "/sessions/{id}/stream",
        "Auto: GET /sessions/{id}/stream.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- skills (generated) --
    route!(
        Get,
        Read,
        "skills.list_bundles",
        "/bundles",
        "Auto: GET /bundles.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.create_bundle",
        "/bundles",
        "Auto: POST /bundles.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "skills.delete_bundle",
        "/bundles/{id}",
        "Auto: DELETE /bundles/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_bundle",
        "/bundles/{id}",
        "Auto: GET /bundles/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.list_bundle_members",
        "/bundles/{id}/skills",
        "Auto: GET /bundles/{id}/skills.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.add_bundle_member",
        "/bundles/{id}/skills",
        "Auto: POST /bundles/{id}/skills.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Delete,
        Write,
        "skills.remove_bundle_member",
        "/bundles/{id}/skills/{skill_id}",
        "Auto: DELETE /bundles/{id}/skills/{skill_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{},"skill_id":{}},"required":["id","skill_id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.list_skills",
        "/skills",
        "Auto: GET /skills.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.create_skill",
        "/skills",
        "Auto: POST /skills.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.resolve_alias",
        "/skills/aliases/resolve",
        "Auto: POST /skills/aliases/resolve.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.cloud_search",
        "/skills/cloud/search",
        "Auto: POST /skills/cloud/search.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.cloud_upload",
        "/skills/cloud/upload",
        "Auto: POST /skills/cloud/upload.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "skills.health",
        "/skills/dashboard/health",
        "Auto: GET /skills/dashboard/health.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "skills.overview",
        "/skills/dashboard/overview",
        "Auto: GET /skills/dashboard/overview.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "skills.stats",
        "/skills/dashboard/stats",
        "Auto: GET /skills/dashboard/stats.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.derive",
        "/skills/derive",
        "Auto: POST /skills/derive.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "skills.evolution_recent",
        "/skills/evolution/recent",
        "Auto: GET /skills/evolution/recent.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.evolve",
        "/skills/evolve",
        "Auto: POST /skills/evolve.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.find_skills",
        "/skills/find",
        "Auto: POST /skills/find.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "skills.upload_skill",
        "/skills/upload",
        "Auto: POST /skills/upload.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "skills.usage_stats",
        "/skills/usage-stats",
        "Auto: GET /skills/usage-stats.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "skills.delete_skill",
        "/skills/{id}",
        "Auto: DELETE /skills/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_skill",
        "/skills/{id}",
        "Auto: GET /skills/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.list_aliases",
        "/skills/{id}/aliases",
        "Auto: GET /skills/{id}/aliases.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.add_alias",
        "/skills/{id}/aliases",
        "Auto: POST /skills/{id}/aliases.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Delete,
        Write,
        "skills.remove_alias",
        "/skills/{id}/aliases/{alias}",
        "Auto: DELETE /skills/{id}/aliases/{alias}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{},"alias":{}},"required":["id","alias"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_deps",
        "/skills/{id}/deps",
        "Auto: GET /skills/{id}/deps.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.detail",
        "/skills/{id}/detail",
        "Auto: GET /skills/{id}/detail.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.fix",
        "/skills/{id}/fix",
        "Auto: POST /skills/{id}/fix.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.judge",
        "/skills/{id}/judge",
        "Auto: POST /skills/{id}/judge.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_judgments",
        "/skills/{id}/judgments",
        "Auto: GET /skills/{id}/judgments.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_lineage",
        "/skills/{id}/lineage",
        "Auto: GET /skills/{id}/lineage.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Delete,
        Write,
        "skills.forget_materialization",
        "/skills/{id}/materialization",
        "Auto: DELETE /skills/{id}/materialization.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_materialization",
        "/skills/{id}/materialization",
        "Auto: GET /skills/{id}/materialization.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.record_materialization",
        "/skills/{id}/materialize",
        "Auto: POST /skills/{id}/materialize.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_tags",
        "/skills/{id}/tags",
        "Auto: GET /skills/{id}/tags.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "skills.record_tool_quality",
        "/tools/quality",
        "Auto: POST /tools/quality.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "skills.get_tool_quality",
        "/tools/quality/{tool_name}",
        "Auto: GET /tools/quality/{tool_name}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"tool_name":{}},"required":["tool_name"]}"#
    ),
    // -- soma (generated) --
    route!(
        Get,
        Read,
        "soma.list_agents",
        "/soma/agents",
        "Auto: GET /soma/agents.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "soma.create_agent",
        "/soma/agents",
        "Register a new Soma agent.",
        ["soma.register", "services.soma_register"],
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "soma.delete_agent",
        "/soma/agents/{id}",
        "Auto: DELETE /soma/agents/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "soma.get_agent",
        "/soma/agents/{id}",
        "Auto: GET /soma/agents/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Patch,
        Write,
        "soma.update_agent",
        "/soma/agents/{id}",
        "Auto: PATCH /soma/agents/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Post,
        Write,
        "soma.log_event",
        "/soma/agents/{id}/log",
        "Auto: POST /soma/agents/{id}/log.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "soma.list_groups",
        "/soma/groups",
        "Auto: GET /soma/groups.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "soma.create_group",
        "/soma/groups",
        "Auto: POST /soma/groups.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "soma.remove_member",
        "/soma/groups/{id}/members/{agent_id}",
        "Auto: DELETE /soma/groups/{id}/members/{agent_id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{},"agent_id":{}},"required":["id","agent_id"]}"#
    ),
    // -- tasks (generated) --
    route!(
        Get,
        Read,
        "tasks.get_feed",
        "/chiasm/feed",
        "Auto: GET /chiasm/feed.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "tasks.list_tasks",
        "/chiasm/tasks",
        "Auto: GET /chiasm/tasks.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "tasks.create_task",
        "/chiasm/tasks",
        "Auto: POST /chiasm/tasks.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "chiasm.generate_plan",
        "/chiasm/tasks/{id}/plan",
        "Generate an LLM execution plan for a Chiasm task.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Post,
        Admin,
        "chiasm.admin_create_key",
        "/chiasm/admin/keys",
        "Mint a per-agent bearer key. Raw key returned exactly once.",
        r#"{"type":"object","properties":{"agent":{"type":"string"}},"required":["agent"]}"#
    ),
    route!(
        Get,
        Admin,
        "chiasm.admin_list_keys",
        "/chiasm/admin/keys",
        "List per-agent bearer keys (no secrets).",
        r#"{"type":"object"}"#
    ),
    route!(
        Delete,
        Admin,
        "chiasm.admin_revoke_key",
        "/chiasm/admin/keys/{id}",
        "Revoke a per-agent bearer key by id.",
        r#"{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}"#
    ),
    route!(
        Delete,
        Write,
        "tasks.delete_task",
        "/tasks/{id}",
        "Auto: DELETE /tasks/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "tasks.get_task",
        "/tasks/{id}",
        "Auto: GET /tasks/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Patch,
        Write,
        "tasks.update_task",
        "/tasks/{id}",
        "Update a Chiasm coordination task.",
        ["tasks.update", "services.chiasm_update_task"],
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- thymus (generated) --
    route!(
        Get,
        Read,
        "thymus.get_drift_events",
        "/thymus/drift-events",
        "Auto: GET /thymus/drift-events.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "thymus.record_drift_event",
        "/thymus/drift-events",
        "Record a thymus drift event.",
        r#"{"type":"object","properties":{"agent":{"type":"string"},"drift_type":{"type":"string"},"signal":{"type":"string"},"session_id":{"type":"string"},"severity":{"type":"string"}},"required":["agent","drift_type","signal"]}"#
    ),
    route!(
        Get,
        Read,
        "thymus.get_metrics",
        "/thymus/metrics",
        "Auto: GET /thymus/metrics.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "thymus.record_metric",
        "/thymus/metrics",
        "Record a thymus metric value.",
        r#"{"type":"object","properties":{"agent":{"type":"string"},"metric":{"type":"string"},"value":{"type":"number"},"tags":{"type":"object"}},"required":["agent","metric","value"]}"#
    ),
    route!(
        Get,
        Read,
        "thymus.list_rubrics",
        "/thymus/rubrics",
        "Auto: GET /thymus/rubrics.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "thymus.create_rubric",
        "/thymus/rubrics",
        "Create a thymus rubric.",
        r#"{"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"criteria":{"type":"object"}},"required":["name","criteria"]}"#
    ),
    route!(
        Delete,
        Write,
        "thymus.delete_rubric",
        "/thymus/rubrics/{id}",
        "Auto: DELETE /thymus/rubrics/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "thymus.get_rubric",
        "/thymus/rubrics/{id}",
        "Auto: GET /thymus/rubrics/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Patch,
        Write,
        "thymus.update_rubric",
        "/thymus/rubrics/{id}",
        "Auto: PATCH /thymus/rubrics/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "thymus.get_session_quality",
        "/thymus/session-quality",
        "Auto: GET /thymus/session-quality.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "thymus.record_session_quality",
        "/thymus/session-quality",
        "Record session quality metrics.",
        r#"{"type":"object","properties":{"session_id":{"type":"string"},"agent":{"type":"string"},"turn_count":{"type":"integer"},"rules_followed":{"type":"array","items":{"type":"string"}},"rules_drifted":{"type":"array","items":{"type":"string"}},"personality_score":{"type":"number"},"rule_compliance_rate":{"type":"number"}},"required":["session_id","agent"]}"#
    ),
    // -- webhooks (generated) --
    route!(
        Get,
        Read,
        "webhooks.list_webhooks",
        "/webhooks",
        "Auto: GET /webhooks.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Post,
        Write,
        "webhooks.create_webhook",
        "/webhooks",
        "Auto: POST /webhooks.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Delete,
        Write,
        "webhooks.delete_webhook",
        "/webhooks/{id}",
        "Auto: DELETE /webhooks/{id}.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    route!(
        Get,
        Read,
        "webhooks.list_dead_letters",
        "/webhooks/{id}/dead-letters",
        "Auto: GET /webhooks/{id}/dead-letters.",
        r#"{"type":"object","additionalProperties":true,"properties":{"id":{}},"required":["id"]}"#
    ),
    // -- well_known (generated) --
    route!(
        Get,
        Read,
        "well_known.agent_card",
        "/.well-known/agent-card.json",
        "Auto: GET /.well-known/agent-card.json.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "well_known.agent_commerce",
        "/.well-known/agent-commerce.json",
        "Auto: GET /.well-known/agent-commerce.json.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
    route!(
        Get,
        Read,
        "well_known.llms_txt",
        "/llms.txt",
        "Auto: GET /llms.txt.",
        r#"{"type":"object","additionalProperties":true}"#
    ),
];

#[cfg(test)]
/// Regression tests for `render_path`. Each test pins a specific behavior
/// of the CWE-74/918 path-injection fix so future refactors cannot silently
/// weaken the encoding contract.
mod tests {
    use super::*;
    use serde_json::json;

    /// Secret-bearing cred routes must be blocked from MCP dispatch (by
    /// canonical name and alias), while ordinary tools stay dispatchable.
    #[test]
    fn mcp_blocks_secret_routes_only() {
        assert!(is_mcp_blocked("admin.cred_resolve"));
        assert!(is_mcp_blocked("admin.cred_proxy"));
        assert!(!is_mcp_blocked("memory.store"));
        assert!(!is_mcp_blocked("memory_store"));
        assert!(!is_mcp_blocked("does.not.exist"));
        // Every blocked name must resolve to a real route in the registry.
        for name in MCP_BLOCKED_ROUTES {
            assert!(
                find_by_name(name).is_some(),
                "blocked route {name} missing from registry"
            );
        }
    }

    /// Path-segment substitution must percent-encode `/`, `?`, `#`, and `%`
    /// so an LLM-supplied argument cannot pivot the request to another route
    /// or graft on a query/fragment. Regression coverage for CWE-74/918.
    #[test]
    fn render_path_blocks_injection_chars() {
        let mut args = json!({ "id": "../admin?evil=1#frag" });
        let out = render_path("/memory/{id}", &mut args).expect("render");
        assert_eq!(out, "/memory/..%2Fadmin%3Fevil=1%23frag");
        assert!(!out.contains("/admin"));
        assert!(!out.contains('?'));
        assert!(!out.contains('#'));
    }

    /// A literal `%` in the input must itself be encoded (`%` -> `%25`) so an
    /// attacker cannot smuggle pre-encoded path traversal through the helper.
    #[test]
    fn render_path_double_encodes_percent() {
        let mut args = json!({ "id": "%2Fadmin" });
        let out = render_path("/memory/{id}", &mut args).expect("render");
        assert_eq!(out, "/memory/%252Fadmin");
    }

    /// CR/LF and other control bytes must be encoded -- defends against
    /// header-injection-via-URL if any downstream layer reflects the path.
    #[test]
    fn render_path_encodes_control_bytes() {
        let mut args = json!({ "id": "a\r\nb" });
        let out = render_path("/memory/{id}", &mut args).expect("render");
        assert_eq!(out, "/memory/a%0D%0Ab");
    }

    /// Numbers and bools pass through as their literal scalar form -- they
    /// cannot contain unsafe characters so encoding is unnecessary.
    #[test]
    fn render_path_passes_scalars_through() {
        let mut args = json!({ "id": 42 });
        assert_eq!(
            render_path("/memory/{id}", &mut args).unwrap(),
            "/memory/42"
        );
        let mut args = json!({ "flag": true });
        assert_eq!(render_path("/x/{flag}", &mut args).unwrap(), "/x/true");
    }

    /// Arrays, objects, and null are not valid path-segment values and must
    /// be rejected loudly rather than coerced via JSON serialization (which
    /// would inject `{`, `}`, `[`, `]`, `,`, and `"` into the URL).
    #[test]
    fn render_path_rejects_non_scalar_values() {
        let mut args = json!({ "id": ["a", "b"] });
        assert!(render_path("/memory/{id}", &mut args).is_err());
        let mut args = json!({ "id": { "nested": true } });
        assert!(render_path("/memory/{id}", &mut args).is_err());
        let mut args = json!({ "id": null });
        assert!(render_path("/memory/{id}", &mut args).is_err());
    }

    /// The consumed key must be removed from args so the dispatcher does not
    /// re-emit it in the request body (where it would shadow the path value).
    #[test]
    fn render_path_consumes_used_keys() {
        let mut args = json!({ "id": "abc", "extra": "kept" });
        render_path("/memory/{id}", &mut args).unwrap();
        let obj = args.as_object().unwrap();
        assert!(!obj.contains_key("id"));
        assert!(obj.contains_key("extra"));
    }

    /// A missing required template key is a hard error -- silent omission
    /// would produce a malformed URL that could collide with another route.
    #[test]
    fn render_path_errors_on_missing_key() {
        let mut args = json!({});
        assert!(render_path("/memory/{id}", &mut args).is_err());
    }

    /// The route registry must not advertise helper endpoints that the server
    /// intentionally removed from its mounted HTTP surface.
    #[test]
    fn registry_excludes_removed_mcp_dispatch_helper() {
        assert!(
            find_by_name("mcp_schema.dispatch").is_none(),
            "stale mcp_schema.dispatch route should not be advertised"
        );
    }

    /// Every route in the registry must have a template key set and an
    /// input-schema property name that matches Anthropic's MCP tool-schema
    /// regex `^[a-zA-Z0-9_.-]{1,64}$`. Axum wildcard templates like
    /// `/_app/{*path}` violate this (leading `*`), and their multi-segment
    /// slash semantics are incompatible with percent-encoded path-segment
    /// substitution -- such routes must not appear in the registry.
    #[test]
    fn registry_property_keys_match_mcp_regex() {
        let key_re = regex_lite_match;
        for route in ROUTES {
            let mut rest = route.path;
            while let Some(open) = rest.find('{') {
                let after = &rest[open + 1..];
                let close = after
                    .find('}')
                    .expect("malformed path template in registry");
                let key = &after[..close];
                assert!(
                    key_re(key),
                    "route {} has template key {:?} that fails Anthropic MCP regex \
                     ^[a-zA-Z0-9_.-]{{1,64}}$",
                    route.name,
                    key
                );
                rest = &after[close + 1..];
            }
        }
    }

    /// Inline minimal matcher for `^[a-zA-Z0-9_.-]{1,64}$` so we do not pull
    /// the `regex` crate just for one test.
    fn regex_lite_match(s: &str) -> bool {
        if s.is_empty() || s.len() > 64 {
            return false;
        }
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    }
}
