//! Local Model Context Protocol transport for Agent-Forge.
//!
//! This module deliberately keeps the Forge database and repository-local
//! operations in one process. The remote Kleos MCP remains the coordination
//! surface; this server owns code-work evidence and Fluency documents.

use crate::db::Database;
use crate::json_io::Output;
use crate::tools;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};

/// Largest accepted MCP message body, preventing unbounded input buffering.
const MAX_MCP_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Wire framing selected from the first message in a stdio session.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Framing {
    /// One compact JSON object per line.
    Ndjson,
    /// LSP-style `Content-Length` headers followed by a JSON body.
    ContentLength,
}

/// One decoded transport item, preserving recoverable JSON parse failures.
enum IncomingMessage {
    /// A syntactically valid JSON value ready for JSON-RPC validation.
    Parsed(Value),
    /// Malformed JSON that receives a parse-error response without ending the session.
    ParseError(String),
}

/// Describe one MCP tool with its complete object schema.
fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object", "properties": properties, "required": required}
    })
}

/// Return the local workflow tools advertised to MCP clients.
pub fn tool_list() -> Vec<Value> {
    vec![
        tool(
            "spec_task",
            "Create a code-work specification before editing.",
            json!({
                "task_description":{"type":"string"}, "task_type":{"type":"string","enum":["feature","bugfix","refactor","enhancement","test","docs"]},
                "acceptance_criteria":{"type":"array","items":{"type":"string"},"minItems":2}, "interface_contract":{"type":"string"},
                "edge_cases":{"type":"array","items":{"type":"string"},"minItems":3}, "files_to_touch":{"type":"array","items":{"type":"string"}},
                "dependencies":{"type":"string"}
            }),
            &[
                "task_description",
                "task_type",
                "acceptance_criteria",
                "interface_contract",
                "edge_cases",
            ],
        ),
        tool(
            "consider_approaches",
            "Record and compare implementation approaches.",
            json!({
                "spec_id":{"type":"string"}, "problem":{"type":"string"}, "chosen_index":{"type":"integer","minimum":0},
                "approaches":{"type":"array","minItems":2,"items":{"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"pros":{"type":"array","items":{"type":"string"}},"cons":{"type":"array","items":{"type":"string"}},"score":{"type":"number"}},"required":["name","description"]}}
            }),
            &["problem", "approaches"],
        ),
        tool(
            "log_hypothesis",
            "Record a debugging hypothesis before a bug fix.",
            json!({"bug_description":{"type":"string"},"hypothesis":{"type":"string"},"confidence":{"type":"number","minimum":0,"maximum":1},"spec_id":{"type":"string"}}),
            &["bug_description", "hypothesis"],
        ),
        tool(
            "log_outcome",
            "Record whether a debugging hypothesis was correct.",
            json!({"hypothesis_id":{"type":"string"},"outcome":{"type":"string","enum":["correct","incorrect","partial"]},"notes":{"type":"string"}}),
            &["hypothesis_id", "outcome"],
        ),
        tool(
            "recall_errors",
            "Recall prior debugging hypotheses before starting a bug fix.",
            json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}}),
            &[],
        ),
        tool(
            "verify",
            "Run verification commands and link evidence to a spec.",
            json!({
                "command":{"type":"string"},"expected_exit_code":{"type":"integer"},"spec_id":{"type":"string"},"criteria_index":{"type":"integer"},"timeout_secs":{"type":"integer","minimum":1},
                "steps":{"type":"array","items":{"type":"object","properties":{"command":{"type":"string"},"expected_exit_code":{"type":"integer"},"label":{"type":"string"}},"required":["command"]}}
            }),
            &[],
        ),
        tool(
            "challenge_code",
            "Build an adversarial review prompt for a source file.",
            json!({"file_path":{"type":"string"},"focus_areas":{"type":"array","items":{"type":"string"}}}),
            &["file_path"],
        ),
        tool(
            "comment_check",
            "Check declaration comment coverage in a source file.",
            json!({"file_path":{"type":"string"}}),
            &["file_path"],
        ),
        tool(
            "checkpoint",
            "Snapshot git state and emit a Fluency slice when spec prose is supplied.",
            json!({
                "name":{"type":"string"},"description":{"type":"string"},"spec_id":{"type":"string"},"intent":{"type":"string"},
                "components":{"type":"array","items":{"type":"string"}},"conditions":{"type":"array","items":{"type":"string"}},"emit":{"type":"boolean"},"repo_root":{"type":"string"}
            }),
            &["name", "repo_root"],
        ),
        tool(
            "rollback",
            "Restore a named checkpoint in its owning repository.",
            json!({"checkpoint_name":{"type":"string"},"repo_root":{"type":"string"}}),
            &["checkpoint_name", "repo_root"],
        ),
        tool(
            "session_learn",
            "Record a reusable discovery in the local Forge database.",
            json!({
                "discovery":{"type":"string","minLength":1}, "context":{"type":"string"},
                "tags":{"type":"array","items":{"type":"string"}}, "capture_as_skill":{"type":"boolean"},
                "spec_id":{"type":"string"}
            }),
            &["discovery"],
        ),
        tool(
            "session_recall",
            "Recall local Forge discoveries by keyword.",
            json!({"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}}),
            &[],
        ),
        tool(
            "session_diff",
            "Summarize the current git diff before completion.",
            json!({"base":{"type":"string"},"repo_root":{"type":"string"}}),
            &["repo_root"],
        ),
        tool(
            "think",
            "Build a structured reasoning prompt.",
            json!({"problem":{"type":"string"},"constraints":{"type":"array","items":{"type":"string"}},"context":{"type":"string"}}),
            &["problem"],
        ),
        tool(
            "declare_unknowns",
            "Partition blocking and non-blocking unknowns.",
            json!({"unknowns":{"type":"array","minItems":1,"items":{"type":"object","properties":{"description":{"type":"string"},"blocking":{"type":"boolean"},"resolution_hint":{"type":"string"}},"required":["description","blocking"]}}}),
            &["unknowns"],
        ),
        tool(
            "update_spec",
            "Transition a specification lifecycle state.",
            json!({"spec_id":{"type":"string"},"status":{"type":"string","enum":["active","completed","failed","blocked"]},"note":{"type":"string"}}),
            &["spec_id", "status"],
        ),
        tool(
            "list_specs",
            "List specifications in the local Forge database.",
            json!({"status":{"type":"string"},"limit":{"type":"integer","minimum":1}}),
            &[],
        ),
        tool(
            "get_spec",
            "Fetch a specification and all linked evidence.",
            json!({"spec_id":{"type":"string"}}),
            &["spec_id"],
        ),
        tool(
            "code_context",
            "Refresh a Git repository index and return a high-confidence token-bounded code context pack.",
            json!({
                "repo_root":{"type":"string"}, "query":{"type":"string"},
                "max_tokens":{"type":"integer","minimum":1,"maximum":16000},
                "focus_paths":{"type":"array","items":{"type":"string"}},
                "recent_paths":{"type":"array","items":{"type":"string"}}
            }),
            &["repo_root", "query"],
        ),
        tool(
            "code_relations",
            "Refresh a Git repository index and return syntax-derived relations for one symbol or path.",
            json!({
                "repo_root":{"type":"string"}, "symbol":{"type":"string"}, "path":{"type":"string"},
                "kinds":{"type":"array","items":{"type":"string"}},
                "direction":{"type":"string","enum":["outgoing","incoming","both"]},
                "limit":{"type":"integer","minimum":1,"maximum":200}
            }),
            &["repo_root"],
        ),
        tool(
            "repo_map",
            "Incrementally refresh and return a token-bounded repository symbol map.",
            json!({
                "path":{"type":"string"}, "focus":{"type":"array","items":{"type":"string"}},
                "max_tokens":{"type":"integer","minimum":1,"maximum":32000}
            }),
            &["path"],
        ),
        tool(
            "search_code",
            "Incrementally refresh and search indexed symbol names.",
            json!({
                "query":{"type":"string"}, "path":{"type":"string"}, "symbol_type":{"type":"string"},
                "limit":{"type":"integer","minimum":1,"maximum":500}
            }),
            &["query"],
        ),
        tool(
            "review",
            "Assemble and optionally write a Fluency review record.",
            json!({"spec_id":{"type":"string"},"repo_root":{"type":"string"},"write":{"type":"boolean"}}),
            &["spec_id", "repo_root"],
        ),
    ]
}

/// Deserialize tool arguments and invoke one typed Agent-Forge function.
fn call_typed<T, F>(db: &Database, arguments: Value, function: F) -> Result<Output, String>
where
    T: DeserializeOwned,
    F: FnOnce(&Database, T) -> tools::ToolResult,
{
    let input = serde_json::from_value(arguments).map_err(|error| error.to_string())?;
    function(db, input).map_err(|error| error.to_string())
}

/// Enforce MCP-only repository context that remains optional for direct CLI calls.
fn validate_mcp_arguments(name: &str, arguments: &Value) -> Result<(), String> {
    if matches!(
        name,
        "checkpoint" | "rollback" | "session_diff" | "review" | "code_context" | "code_relations"
    ) && arguments
        .get("repo_root")
        .and_then(Value::as_str)
        .is_none_or(|repo_root| repo_root.trim().is_empty())
    {
        return Err(format!("{name} requires a non-empty repo_root"));
    }
    Ok(())
}

/// Dispatch one advertised tool against the shared local database.
fn call_tool(db: &Database, name: &str, arguments: Value) -> Result<Output, String> {
    validate_mcp_arguments(name, &arguments)?;
    match name {
        "spec_task" => call_typed(db, arguments, tools::spec::spec_task),
        "consider_approaches" => call_typed(db, arguments, tools::approaches::consider_approaches),
        "log_hypothesis" => call_typed(db, arguments, tools::hypothesis::log_hypothesis),
        "log_outcome" => call_typed(db, arguments, tools::hypothesis::log_outcome),
        "recall_errors" => call_typed(db, arguments, tools::hypothesis::recall_errors),
        "verify" => call_typed(db, arguments, tools::verify::verify),
        "challenge_code" => call_typed(db, arguments, tools::verify::challenge_code),
        "comment_check" => call_typed(db, arguments, tools::comments::comment_check),
        "checkpoint" => call_typed(db, arguments, tools::session::checkpoint),
        "rollback" => call_typed(db, arguments, tools::session::rollback),
        "session_learn" => call_typed(db, arguments, tools::session::session_learn),
        "session_recall" => call_typed(db, arguments, tools::session::session_recall),
        "session_diff" => call_typed(db, arguments, tools::verify::session_diff),
        "think" => call_typed(db, arguments, tools::think::think),
        "declare_unknowns" => call_typed(db, arguments, tools::think::declare_unknowns),
        "update_spec" => call_typed(db, arguments, tools::spec::update_spec),
        "list_specs" => call_typed(db, arguments, tools::spec::list_specs),
        "get_spec" => call_typed(db, arguments, tools::spec::get_spec),
        "code_context" => call_typed(db, arguments, tools::code_context::code_context),
        "code_relations" => call_typed(db, arguments, tools::code_context::code_relations),
        "repo_map" => call_typed(db, arguments, tools::ast::repo_map::repo_map),
        "search_code" => call_typed(db, arguments, tools::ast::search::search_code),
        "review" => call_typed(db, arguments, tools::emit::review),
        _ => Err(format!("unknown Agent-Forge tool: {name}")),
    }
}

/// Build a JSON-RPC error response.
fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

/// Convert an Agent-Forge output into the MCP content and structured-content contract.
fn tool_result(output: Output) -> Value {
    let is_error = !output.success;
    let structured = serde_json::to_value(&output).expect("Output serialization cannot fail");
    json!({
        "content": [{"type": "text", "text": structured.to_string()}],
        "structuredContent": structured,
        "isError": is_error
    })
}

/// Convert a tool failure into a normal MCP tool result marked as an error.
fn tool_failure(message: String) -> Value {
    json!({"content": [{"type": "text", "text": message}], "isError": true})
}

/// Handle one non-batch MCP JSON-RPC request, returning no response for valid
/// notifications.
fn handle_request(db: &Database, request: Value) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if !request.is_object() || request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(rpc_error(id, -32600, "invalid JSON-RPC request"));
    }
    let method = match request.get("method").and_then(Value::as_str) {
        Some(method) => method,
        None => return Some(rpc_error(id, -32600, "missing JSON-RPC method")),
    };
    if request.get("id").is_none() {
        return None;
    }
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "agent-forge-mcp", "version": env!("CARGO_PKG_VERSION")}
        }),
        "ping" => json!({}),
        "tools/list" => json!({"tools": tool_list()}),
        "tools/call" => {
            let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, -32602, "tools/call requires params.name"));
            };
            if !tool_list()
                .iter()
                .any(|tool| tool["name"].as_str() == Some(name))
            {
                return Some(rpc_error(id, -32602, format!("unknown tool: {name}")));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(db, name, arguments) {
                Ok(output) => tool_result(output),
                Err(error) => tool_failure(error),
            }
        }
        _ => return Some(rpc_error(id, -32601, format!("method not found: {method}"))),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// Collapse zero or more batch-member responses into the JSON-RPC batch shape.
fn batch_response(responses: Vec<Value>) -> Option<Value> {
    if responses.is_empty() {
        None
    } else {
        Some(Value::Array(responses))
    }
}

/// Handle one JSON-RPC message, including batches, without applying connection
/// lifecycle rules. The stdio server wraps this dispatcher with session state.
pub fn handle_jsonrpc(db: &Database, request: Value) -> Option<Value> {
    match request {
        Value::Array(requests) if requests.is_empty() => {
            Some(rpc_error(Value::Null, -32600, "empty JSON-RPC batch"))
        }
        Value::Array(requests) => batch_response(
            requests
                .into_iter()
                .filter_map(|request| handle_request(db, request))
                .collect(),
        ),
        request => handle_request(db, request),
    }
}

/// Initialization state for one stdio MCP connection.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SessionState {
    /// No initialize request has completed.
    AwaitingInitialize,
    /// Initialize has returned and the client notification is pending.
    AwaitingInitializedNotification,
    /// Capability negotiation is complete and tools may execute.
    Ready,
}

/// Handle a single request while enforcing the MCP initialization lifecycle.
fn handle_session_request(
    db: &Database,
    request: Value,
    state: &mut SessionState,
) -> Option<Value> {
    if !request.is_object()
        || request.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || request.get("method").and_then(Value::as_str).is_none()
    {
        return handle_request(db, request);
    }
    let method = request.get("method").and_then(Value::as_str);
    let id = request.get("id").cloned();

    match *state {
        SessionState::AwaitingInitialize => match method {
            Some("initialize") => {
                let Some(id) = id else {
                    return Some(rpc_error(
                        Value::Null,
                        -32600,
                        "initialize must be a request",
                    ));
                };
                if request
                    .get("params")
                    .and_then(|params| params.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .is_none()
                {
                    return Some(rpc_error(
                        id,
                        -32602,
                        "initialize requires params.protocolVersion",
                    ));
                }
                let response = handle_request(db, request);
                *state = SessionState::AwaitingInitializedNotification;
                response
            }
            Some("ping") => handle_request(db, request),
            _ => id.map(|id| rpc_error(id, -32002, "server is not initialized")),
        },
        SessionState::AwaitingInitializedNotification => match method {
            Some("notifications/initialized") if id.is_none() => {
                *state = SessionState::Ready;
                None
            }
            Some("ping") => handle_request(db, request),
            _ => id.map(|id| {
                rpc_error(
                    id,
                    -32002,
                    "client initialized notification has not been received",
                )
            }),
        },
        SessionState::Ready => {
            if method == Some("initialize") {
                return id.map(|id| rpc_error(id, -32600, "server is already initialized"));
            }
            handle_request(db, request)
        }
    }
}

/// Handle one connection-scoped message and reject batches until initialization
/// has completed as required by the negotiated MCP protocol version.
fn handle_session_message(
    db: &Database,
    request: Value,
    state: &mut SessionState,
) -> Option<Value> {
    match request {
        Value::Array(requests) if requests.is_empty() => {
            Some(rpc_error(Value::Null, -32600, "empty JSON-RPC batch"))
        }
        Value::Array(requests) if *state != SessionState::Ready => batch_response(
            requests
                .into_iter()
                .filter_map(|request| {
                    request.get("id").cloned().map(|id| {
                        rpc_error(
                            id,
                            -32002,
                            "initialize must complete before JSON-RPC batches",
                        )
                    })
                })
                .collect(),
        ),
        Value::Array(requests) => batch_response(
            requests
                .into_iter()
                .filter_map(|request| handle_session_request(db, request, state))
                .collect(),
        ),
        request => handle_session_request(db, request, state),
    }
}

/// Read a capped line so malformed peers cannot grow memory without bound.
fn read_line_capped<R: BufRead>(reader: &mut R) -> Result<Option<String>, String> {
    let mut output = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|error| error.to_string())?;
        if available.is_empty() {
            return if output.is_empty() {
                Ok(None)
            } else {
                String::from_utf8(output)
                    .map(Some)
                    .map_err(|error| error.to_string())
            };
        }
        let (length, complete) = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| (index + 1, true))
            .unwrap_or((available.len(), false));
        if output.len() + length > MAX_MCP_MESSAGE_SIZE {
            return Err(format!("line exceeds max {MAX_MCP_MESSAGE_SIZE} bytes"));
        }
        output.extend_from_slice(&available[..length]);
        reader.consume(length);
        if complete {
            return String::from_utf8(output)
                .map(Some)
                .map_err(|error| error.to_string());
        }
    }
}

/// Parse a Content-Length header value and enforce the message-size cap.
fn parse_content_length(header: &str) -> Result<usize, String> {
    let length = header
        .strip_prefix("Content-Length:")
        .ok_or_else(|| "not a Content-Length header".to_string())?
        .trim()
        .parse::<usize>()
        .map_err(|error| error.to_string())?;
    if length > MAX_MCP_MESSAGE_SIZE {
        return Err(format!(
            "Content-Length {length} exceeds max {MAX_MCP_MESSAGE_SIZE}"
        ));
    }
    Ok(length)
}

/// Decode one JSON byte sequence while retaining parse errors as recoverable input.
fn decode_json(bytes: &[u8]) -> IncomingMessage {
    match serde_json::from_slice(bytes) {
        Ok(value) => IncomingMessage::Parsed(value),
        Err(error) => IncomingMessage::ParseError(error.to_string()),
    }
}

/// Read and decode an exact Content-Length body.
fn read_body<R: Read>(reader: &mut R, length: usize) -> Result<IncomingMessage, String> {
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| error.to_string())?;
    Ok(decode_json(&body))
}

/// Read the first request and detect the peer's framing mode.
fn read_first<R: BufRead>(reader: &mut R) -> Result<Option<(IncomingMessage, Framing)>, String> {
    loop {
        let Some(line) = read_line_capped(reader)? else {
            return Ok(None);
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("Content-Length:") {
            let length = parse_content_length(trimmed)?;
            while read_line_capped(reader)?.is_some_and(|header| !header.trim().is_empty()) {}
            return read_body(reader, length).map(|value| Some((value, Framing::ContentLength)));
        }
        return Ok(Some((decode_json(trimmed.as_bytes()), Framing::Ndjson)));
    }
}

/// Read the next request using the negotiated framing.
fn read_next<R: BufRead>(
    reader: &mut R,
    framing: Framing,
) -> Result<Option<IncomingMessage>, String> {
    match framing {
        Framing::Ndjson => loop {
            let Some(line) = read_line_capped(reader)? else {
                return Ok(None);
            };
            if !line.trim().is_empty() {
                return Ok(Some(decode_json(line.trim().as_bytes())));
            }
        },
        Framing::ContentLength => {
            let mut length = None;
            loop {
                let Some(line) = read_line_capped(reader)? else {
                    return Ok(None);
                };
                if line.trim().is_empty() {
                    break;
                }
                if line.trim().starts_with("Content-Length:") {
                    length = Some(parse_content_length(line.trim())?);
                }
            }
            read_body(
                reader,
                length.ok_or_else(|| "missing Content-Length header".to_string())?,
            )
            .map(Some)
        }
    }
}

/// Dispatch one decoded item or emit a JSON-RPC parse error for malformed JSON.
fn process_incoming<W: Write>(
    db: &Database,
    incoming: IncomingMessage,
    state: &mut SessionState,
    writer: &mut W,
    framing: Framing,
) -> Result<(), String> {
    let response = match incoming {
        IncomingMessage::Parsed(request) => handle_session_message(db, request, state),
        IncomingMessage::ParseError(error) => Some(rpc_error(
            Value::Null,
            -32700,
            format!("parse error: {error}"),
        )),
    };
    if let Some(response) = response {
        write_response(writer, &response, framing)?;
    }
    Ok(())
}

/// Write one response using the session's negotiated framing.
fn write_response<W: Write>(writer: &mut W, value: &Value, framing: Framing) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if framing == Framing::ContentLength {
        write!(writer, "Content-Length: {}\r\n\r\n", body.len())
            .map_err(|error| error.to_string())?;
    }
    writer.write_all(&body).map_err(|error| error.to_string())?;
    if framing == Framing::Ndjson {
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
    }
    writer.flush().map_err(|error| error.to_string())
}

/// Serve MCP requests over arbitrary buffered streams until input reaches EOF.
pub fn serve<R: BufRead, W: Write>(
    db: &Database,
    reader: &mut R,
    writer: &mut W,
) -> Result<(), String> {
    let Some((first, framing)) = read_first(reader)? else {
        return Ok(());
    };
    let mut state = SessionState::AwaitingInitialize;
    process_incoming(db, first, &mut state, writer, framing)?;
    loop {
        match read_next(reader, framing) {
            Ok(Some(request)) => process_incoming(db, request, &mut state, writer, framing)?,
            Ok(None) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Serve the local MCP protocol over locked process stdin and stdout.
pub fn serve_stdio(db: &Database) -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(db, &mut BufReader::new(stdin.lock()), &mut stdout.lock())
}

#[cfg(test)]
/// Protocol and shared-database tests for the local MCP surface.
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;
    use tempfile::tempdir;

    /// Initialize a one-commit repository and return its current object ID.
    fn initialize_git_repo(repo: &Path) -> String {
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo)
            .status()
            .unwrap()
            .success());
        std::fs::write(repo.join("tracked.txt"), "initial").unwrap();
        assert!(Command::new("git")
            .args(["add", "tracked.txt"])
            .current_dir(repo)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args([
                "-c",
                "user.name=Agent Forge Test",
                "-c",
                "user.email=agent-forge@example.invalid",
                "commit",
                "-q",
                "-m",
                "initial",
            ])
            .current_dir(repo)
            .status()
            .unwrap()
            .success());
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(repo)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    /// Build a standards-compliant initialize request.
    fn initialize_request(id: i64) -> Value {
        json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-03-26",
                "capabilities":{},
                "clientInfo":{"name":"agent-forge-test","version":"1"}
            }
        })
    }

    /// Prefix operation messages with the complete MCP initialization exchange.
    fn initialized_ndjson(messages: Vec<Value>) -> Vec<u8> {
        let mut all = vec![
            initialize_request(1),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        ];
        all.extend(messages);
        let mut body = all
            .into_iter()
            .map(|message| serde_json::to_string(&message).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        body.push('\n');
        body.into_bytes()
    }

    /// Build a JSON-RPC tools/call request.
    fn request(id: i64, name: &str, arguments: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
    }

    /// Extract the structured result from a successful JSON-RPC response.
    fn structured(response: Value) -> Value {
        response["result"]["structuredContent"].clone()
    }

    /// Initialization and tool discovery advertise the Fluency operations.
    #[test]
    fn initializes_and_lists_fluency_tools() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let response =
            handle_jsonrpc(&db, json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})).unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(names.contains(&"checkpoint"));
        assert!(names.contains(&"review"));
        assert!(names.contains(&"session_learn"));
        assert!(names.contains(&"session_recall"));
        assert!(names.contains(&"recall_errors"));
        assert!(names.contains(&"rollback"));
        assert!(names.contains(&"spec_task"));
        assert!(names.contains(&"code_context"));
        assert!(names.contains(&"code_relations"));
        assert!(names.contains(&"repo_map"));
        assert!(names.contains(&"search_code"));
        assert_eq!(names.len(), 23);
    }

    /// Learning calls persist and recall discoveries through the shared database.
    #[test]
    fn learns_and_recalls_from_one_database() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let learned = structured(
            handle_jsonrpc(
                &db,
                request(
                    1,
                    "session_learn",
                    json!({"discovery":"MCP learning continuity marker","tags":["mcp"]}),
                ),
            )
            .unwrap(),
        );
        assert_eq!(learned["success"], true);
        let recalled = structured(
            handle_jsonrpc(
                &db,
                request(2, "session_recall", json!({"query":"continuity"})),
            )
            .unwrap(),
        );
        assert_eq!(recalled["success"], true);
        assert_eq!(
            recalled["data"]["results"][0]["discovery"],
            "MCP learning continuity marker"
        );
    }

    /// Spec creation, checkpoint emission, and review share one local database.
    #[test]
    fn emits_and_reviews_from_one_database() {
        let dir = tempdir().unwrap();
        let expected_ref = initialize_git_repo(dir.path());
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let spec = structured(handle_jsonrpc(&db, request(1, "spec_task", json!({
            "task_description":"Teach a local MCP slice", "task_type":"feature",
            "acceptance_criteria":["MCP works", "Fluency emits"],
            "interface_contract":"Local stdio MCP", "edge_cases":["EOF", "bad JSON", "hollow prose"]
        }))).unwrap());
        let spec_id = spec["id"].as_str().unwrap();
        let checkpoint = structured(handle_jsonrpc(&db, request(2, "checkpoint", json!({
            "name":"local-mcp-test", "spec_id":spec_id, "intent":"Prove local state continuity",
            "components":["The local server retains one database across MCP calls."],
            "conditions":["Fluency must be compiled in."], "repo_root":dir.path()
        }))).unwrap());
        assert_eq!(checkpoint["success"], true);
        let checkpoint_id = checkpoint["id"].as_str().unwrap();
        let stored_ref: Option<String> = db
            .conn()
            .query_row(
                "SELECT git_ref FROM checkpoints WHERE id = ?1",
                [checkpoint_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_ref.as_deref(), Some(expected_ref.as_str()));
        let slice_path = checkpoint["data"]["slice_path"].as_str().unwrap();
        let review = structured(
            handle_jsonrpc(
                &db,
                request(
                    3,
                    "review",
                    json!({
                        "spec_id":spec_id, "repo_root":dir.path()
                    }),
                ),
            )
            .unwrap(),
        );
        assert_eq!(review["success"], true);
        assert!(std::path::Path::new(slice_path).is_file());
        assert!(dir
            .path()
            .join("docs/agent-forge/work/teach-a-local-mcp-slice/record.md")
            .is_file());
    }

    /// Unknown methods and tools return errors without poisoning later calls.
    #[test]
    fn errors_do_not_terminate_dispatch() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let method =
            handle_jsonrpc(&db, json!({"jsonrpc":"2.0","id":1,"method":"missing"})).unwrap();
        assert_eq!(method["error"]["code"], -32601);
        let tool = handle_jsonrpc(&db, request(2, "missing", json!({}))).unwrap();
        assert_eq!(tool["error"]["code"], -32602);
        let invalid = handle_jsonrpc(&db, request(3, "session_learn", json!({}))).unwrap();
        assert_eq!(invalid["result"]["isError"], true);
        let missing_root =
            handle_jsonrpc(&db, request(4, "session_diff", json!({"base":"HEAD"}))).unwrap();
        assert_eq!(missing_root["result"]["isError"], true);
        assert!(missing_root["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("non-empty repo_root"));
        let ping = handle_jsonrpc(&db, json!({"jsonrpc":"2.0","id":3,"method":"ping"})).unwrap();
        assert!(ping.get("result").is_some());
    }

    /// A failed verification is a failed MCP tool result, not a successful call
    /// containing a private Agent-Forge failure bit.
    #[test]
    fn failed_verify_sets_mcp_error_flag() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let response =
            handle_jsonrpc(&db, request(1, "verify", json!({"command":"false"}))).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["structuredContent"]["success"], false);
    }

    /// Completion tools return MCP failures when required Git evidence does
    /// not exist, and recall enforces its typed result bound.
    #[test]
    fn completion_tools_fail_without_evidence() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let checkpoint = handle_jsonrpc(
            &db,
            request(
                1,
                "checkpoint",
                json!({"name":"no-ref","repo_root":dir.path()}),
            ),
        )
        .unwrap();
        assert_eq!(checkpoint["result"]["isError"], true);
        let checkpoint_rows: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM checkpoints", [], |row| row.get(0))
            .unwrap();
        assert_eq!(checkpoint_rows, 0);

        let rollback = handle_jsonrpc(
            &db,
            request(
                2,
                "rollback",
                json!({"checkpoint_name":"no-ref","repo_root":dir.path()}),
            ),
        )
        .unwrap();
        assert_eq!(rollback["result"]["isError"], true);

        let session_diff = handle_jsonrpc(
            &db,
            request(
                3,
                "session_diff",
                json!({"base":"HEAD","repo_root":dir.path()}),
            ),
        )
        .unwrap();
        assert_eq!(session_diff["result"]["isError"], true);

        let recall =
            handle_jsonrpc(&db, request(4, "recall_errors", json!({"limit":101}))).unwrap();
        assert_eq!(recall["result"]["isError"], true);
    }

    /// Session diff reads the explicitly requested repository instead of the
    /// MCP server process directory.
    #[test]
    fn session_diff_uses_repo_root() {
        let dir = tempdir().unwrap();
        initialize_git_repo(dir.path());
        std::fs::write(dir.path().join("tracked.txt"), "changed").unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let response = handle_jsonrpc(
            &db,
            request(
                1,
                "session_diff",
                json!({"base":"HEAD","repo_root":dir.path()}),
            ),
        )
        .unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(
            response["result"]["structuredContent"]["data"]["files"][0],
            "tracked.txt"
        );
    }

    /// Tool calls are refused until initialization and its completion
    /// notification have both arrived.
    #[test]
    fn rejects_tools_before_initialization() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let input = initialized_ndjson(vec![json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})]);
        let mut prefixed = b"{\"jsonrpc\":\"2.0\",\"id\":0,\"method\":\"tools/list\"}\n".to_vec();
        prefixed.extend(input);
        let mut reader = BufReader::new(prefixed.as_slice());
        let mut output = Vec::new();
        serve(&db, &mut reader, &mut output).unwrap();
        let responses: Vec<Value> = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses[0]["error"]["code"], -32002);
        assert_eq!(responses[1]["result"]["protocolVersion"], "2025-03-26");
        assert!(responses[2]["result"]["tools"].is_array());
    }

    /// NDJSON transport processes notifications and subsequent requests.
    #[test]
    fn serves_ndjson_until_eof() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let input = initialized_ndjson(vec![json!({"jsonrpc":"2.0","id":2,"method":"ping"})]);
        let mut reader = BufReader::new(input.as_slice());
        let mut output = Vec::new();
        serve(&db, &mut reader, &mut output).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&output).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(serde_json::from_str::<Value>(lines[1]).unwrap()["id"], 2);
    }

    /// A malformed first NDJSON request reports a parse error and the stream continues.
    #[test]
    fn malformed_ndjson_does_not_terminate_stream() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let mut input = b"{broken}\n".to_vec();
        input.extend(initialized_ndjson(vec![
            json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
        ]));
        let mut reader = BufReader::new(input.as_slice());
        let mut output = Vec::new();
        serve(&db, &mut reader, &mut output).unwrap();
        let responses: Vec<Value> = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert_eq!(responses[2]["id"], 2);
    }

    /// Ready sessions execute JSON-RPC batches and omit notification entries
    /// from the batch response.
    #[test]
    fn serves_jsonrpc_batches() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let input = initialized_ndjson(vec![json!([
            {"jsonrpc":"2.0","id":2,"method":"ping"},
            {"jsonrpc":"2.0","method":"notifications/cancelled"},
            {"jsonrpc":"2.0","id":3,"method":"ping"}
        ])]);
        let mut reader = BufReader::new(input.as_slice());
        let mut output = Vec::new();
        serve(&db, &mut reader, &mut output).unwrap();
        let responses: Vec<Value> = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[1].as_array().unwrap().len(), 2);
        assert_eq!(responses[1][0]["id"], 2);
        assert_eq!(responses[1][1]["id"], 3);
    }

    /// Content-Length framing is detected and preserved in the response.
    #[test]
    fn serves_content_length_framing() {
        let dir = tempdir().unwrap();
        let db = Database::open(&dir.path().join("forge.db")).unwrap();
        let body = b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}";
        let input = format!(
            "Content-Length: {}\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        let mut reader = BufReader::new(input.as_bytes());
        let mut output = Vec::new();
        serve(&db, &mut reader, &mut output).unwrap();
        let rendered = std::str::from_utf8(&output).unwrap();
        let (_, response_body) = rendered.split_once("\r\n\r\n").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(response_body).unwrap()["id"],
            7
        );
    }
}
