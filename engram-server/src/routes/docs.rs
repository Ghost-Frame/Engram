// ============================================================================
// OpenAPI spec + Swagger UI
// ============================================================================
//
// Publishes:
//   GET /openapi.json        -- machine-readable spec
//   GET /docs/openapi.json   -- alias
//   GET /docs                -- Swagger UI (HTML, pulls assets from jsDelivr)
//
// The spec is hand-curated: paths list every registered route, with tag +
// summary + common parameters + request/response shapes for the core types.
// Detailed per-field definitions for the domain objects (Memory,
// SearchResult, etc.) live in the `GET /schema` routes and are also referenced
// from the components/schemas block below.

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Map, Value};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/openapi.json", get(openapi))
        .route("/docs/openapi.json", get(openapi))
        .route("/docs", get(swagger_ui))
        .route("/docs/", get(swagger_ui))
}

async fn openapi() -> Json<Value> {
    Json(build_openapi_spec())
}

async fn swagger_ui() -> Response {
    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <title>Engram API -- Swagger UI</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css" />
  <style>body { margin: 0; }</style>
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
  <script>
    window.ui = SwaggerUIBundle({
      url: '/openapi.json',
      dom_id: '#swagger-ui',
      deepLinking: true,
      presets: [SwaggerUIBundle.presets.apis],
      layout: 'BaseLayout'
    });
  </script>
</body>
</html>"#;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn build_openapi_spec() -> Value {
    let mut paths = Map::new();
    for (path, methods, summary, tag) in route_specs() {
        let mut item = Map::new();
        for method in methods.iter() {
            item.insert(
                (*method).to_string(),
                operation_for(path, method, summary, tag),
            );
        }
        paths.insert(path.to_string(), Value::Object(item));
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Engram Memory API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "OpenAPI surface for the Engram Rust server. \
                Per-field definitions of core domain objects (Memory, SearchResult, \
                MemoryLink, services) are also served by the live schema \
                introspection endpoints: `GET /schema`, `GET /schema/memory`, \
                `GET /schema/services`, `GET /schema/graph`."
        },
        "servers": [{ "url": "/" }],
        "security": [{ "BearerAuth": [] }],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "BearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "API key",
                    "description": "Provide the tenant API key in `Authorization: Bearer <key>`."
                }
            },
            "parameters": {
                "Limit": {
                    "name": "limit",
                    "in": "query",
                    "required": false,
                    "schema": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 },
                    "description": "Maximum number of records to return."
                },
                "Offset": {
                    "name": "offset",
                    "in": "query",
                    "required": false,
                    "schema": { "type": "integer", "minimum": 0, "default": 0 },
                    "description": "Offset for offset-based pagination."
                },
                "Cursor": {
                    "name": "cursor",
                    "in": "query",
                    "required": false,
                    "schema": { "type": "string" },
                    "description": "Opaque cursor token for forward pagination."
                }
            },
            "schemas": {
                "Error": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {
                        "error": { "type": "string", "description": "Human-readable error message." },
                        "code":  { "type": "string", "description": "Optional stable error code." }
                    }
                },
                "PageMeta": {
                    "type": "object",
                    "description": "Standard pagination metadata. See engram-lib::pagination.",
                    "required": ["has_more"],
                    "properties": {
                        "next_cursor": { "type": "string", "description": "Forward cursor for the next page; absent at end." },
                        "has_more":    { "type": "boolean" },
                        "total":       { "type": "integer", "format": "int64", "description": "Total match count, when cheap to compute." }
                    }
                },
                "Envelope": {
                    "type": "object",
                    "description": "Standard success envelope for single-resource responses.",
                    "required": ["data"],
                    "properties": {
                        "data": { "description": "Resource payload (shape depends on the endpoint)." },
                        "meta": { "type": "object", "description": "Optional arbitrary metadata (timings, warnings)." }
                    }
                },
                "ListEnvelope": {
                    "type": "object",
                    "description": "Standard list response envelope with pagination.",
                    "required": ["data", "meta"],
                    "properties": {
                        "data": { "type": "array", "items": { "type": "object" } },
                        "meta": { "$ref": "#/components/schemas/PageMeta" }
                    }
                },
                "StoreRequest": {
                    "type": "object",
                    "required": ["content"],
                    "properties": {
                        "content":    { "type": "string", "maxLength": 102400, "description": "Memory body." },
                        "category":   { "type": "string", "description": "Category label (e.g. fact, task, preference)." },
                        "importance": { "type": "integer", "minimum": 1, "maximum": 10 },
                        "tags":       { "type": "array", "items": { "type": "string" } },
                        "source":     { "type": "string" },
                        "session_id": { "type": "string" },
                        "agent":      { "type": "string" }
                    }
                },
                "StoreResponse": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id":      { "type": "integer", "format": "int64" },
                        "status":  { "type": "string", "enum": ["stored"] }
                    }
                },
                "SearchRequest": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query":    { "type": "string", "maxLength": 4096 },
                        "limit":    { "type": "integer", "minimum": 1, "maximum": 100 },
                        "tags":     { "type": "array", "items": { "type": "string" } },
                        "category": { "type": "string" }
                    }
                },
                "Memory": {
                    "type": "object",
                    "description": "See GET /schema/memory for full field definitions.",
                    "properties": {
                        "id":         { "type": "integer", "format": "int64" },
                        "content":    { "type": "string" },
                        "category":   { "type": "string" },
                        "importance": { "type": "integer" },
                        "tags":       { "type": "array", "items": { "type": "string" } },
                        "created_at": { "type": "string", "format": "date-time" }
                    }
                },
                "SearchResult": {
                    "type": "object",
                    "description": "See GET /schema/memory related_shapes.SearchResult for full fields.",
                    "properties": {
                        "id":      { "type": "integer", "format": "int64" },
                        "content": { "type": "string" },
                        "score":   { "type": "number", "format": "float" }
                    }
                },
                "SearchResponse": {
                    "type": "object",
                    "properties": {
                        "results": { "type": "array", "items": { "$ref": "#/components/schemas/SearchResult" } },
                        "total":   { "type": "integer" }
                    }
                },
                "MemoryLink": {
                    "type": "object",
                    "description": "See GET /schema/graph for edge definition.",
                    "properties": {
                        "source_id":   { "type": "integer", "format": "int64" },
                        "target_id":   { "type": "integer", "format": "int64" },
                        "link_type":   { "type": "string" },
                        "strength":    { "type": "number", "format": "float" }
                    }
                },
                "HealthResponse": {
                    "type": "object",
                    "properties": {
                        "status":  { "type": "string" },
                        "version": { "type": "string" }
                    }
                }
            },
            "responses": {
                "Unauthorized": {
                    "description": "Missing or invalid API key.",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
                },
                "BadRequest": {
                    "description": "Request validation failed.",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
                },
                "NotFound": {
                    "description": "Resource not found.",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } }
                }
            }
        }
    })
}

/// Build a single operation object. Adds path/query parameters, request
/// body for write methods, and typed responses for well-known routes.
fn operation_for(path: &str, method: &str, summary: &str, tag: &str) -> Value {
    let mut op = Map::new();
    op.insert("summary".into(), Value::String(summary.into()));
    op.insert("tags".into(), json!([tag]));

    let mut params: Vec<Value> = Vec::new();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        if let Some(name) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            let schema_type = if name.ends_with("_id") || name == "id" || name == "mid" {
                "integer"
            } else {
                "string"
            };
            let mut schema = Map::new();
            schema.insert("type".into(), Value::String(schema_type.into()));
            if schema_type == "integer" {
                schema.insert("format".into(), Value::String("int64".into()));
            }
            params.push(json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": Value::Object(schema),
            }));
        }
    }

    let is_list = matches!(method, "get")
        && (path.ends_with('s')
            || path.ends_with("/list")
            || path.ends_with("/feed")
            || path.ends_with("/events")
            || path.ends_with("/messages")
            || path.ends_with("/members"));
    if is_list {
        params.push(json!({ "$ref": "#/components/parameters/Limit" }));
        params.push(json!({ "$ref": "#/components/parameters/Offset" }));
        params.push(json!({ "$ref": "#/components/parameters/Cursor" }));
    }

    if !params.is_empty() {
        op.insert("parameters".into(), Value::Array(params));
    }

    if matches!(method, "post" | "put" | "patch") {
        let body_schema = request_body_for(path, method);
        op.insert(
            "requestBody".into(),
            json!({
                "required": true,
                "content": {
                    "application/json": { "schema": body_schema }
                }
            }),
        );
    }

    let success = response_schema_for(path, method);
    op.insert(
        "responses".into(),
        json!({
            "200": {
                "description": "Successful response",
                "content": { "application/json": { "schema": success } }
            },
            "400": { "$ref": "#/components/responses/BadRequest" },
            "401": { "$ref": "#/components/responses/Unauthorized" },
            "404": { "$ref": "#/components/responses/NotFound" }
        }),
    );

    Value::Object(op)
}

fn request_body_for(path: &str, _method: &str) -> Value {
    match path {
        "/store" | "/memory" | "/memories" => {
            json!({ "$ref": "#/components/schemas/StoreRequest" })
        }
        "/search" | "/memories/search" | "/search/explain" | "/search/faceted" | "/recall" => {
            json!({ "$ref": "#/components/schemas/SearchRequest" })
        }
        _ => json!({ "type": "object", "additionalProperties": true }),
    }
}

fn response_schema_for(path: &str, method: &str) -> Value {
    match (path, method) {
        ("/health", _)
        | ("/live", _)
        | ("/ready", _)
        | ("/health/live", _)
        | ("/health/ready", _) => {
            json!({ "$ref": "#/components/schemas/HealthResponse" })
        }
        ("/store", _) | ("/memory", "post") | ("/memories", "post") => {
            json!({ "$ref": "#/components/schemas/StoreResponse" })
        }
        ("/search", _)
        | ("/memories/search", _)
        | ("/search/explain", _)
        | ("/search/faceted", _)
        | ("/recall", _) => json!({ "$ref": "#/components/schemas/SearchResponse" }),
        _ => json!({ "type": "object", "additionalProperties": true }),
    }
}

fn route_specs() -> &'static [(
    &'static str,
    &'static [&'static str],
    &'static str,
    &'static str,
)] {
    &[
        ("/health", &["get"], "Health check", "System"),
        ("/live", &["get"], "Liveness check", "System"),
        ("/ready", &["get"], "Readiness check", "System"),
        ("/health/live", &["get"], "Liveness probe", "System"),
        ("/health/ready", &["get"], "Readiness probe", "System"),
        ("/openapi.json", &["get"], "OpenAPI specification", "Docs"),
        (
            "/docs/openapi.json",
            &["get"],
            "OpenAPI specification",
            "Docs",
        ),
        ("/docs", &["get"], "Swagger UI", "Docs"),
        ("/schema", &["get"], "Schema index", "Schema"),
        (
            "/schema/memory",
            &["get"],
            "Memory + SearchResult schema",
            "Schema",
        ),
        (
            "/schema/services",
            &["get"],
            "Service module catalog",
            "Schema",
        ),
        (
            "/schema/graph",
            &["get"],
            "Graph node/edge schema",
            "Schema",
        ),
        ("/store", &["post"], "Store memory", "Memory"),
        ("/memory", &["post"], "Store memory", "Memory"),
        ("/memories", &["post"], "Store memory", "Memory"),
        ("/search", &["post"], "Search memories", "Memory"),
        ("/memories/search", &["post"], "Search memories", "Memory"),
        (
            "/search/explain",
            &["post"],
            "Search with per-result score breakdown + stage timings",
            "Memory",
        ),
        (
            "/search/faceted",
            &["post"],
            "Faceted search with multi-tag + facet aggregation",
            "Memory",
        ),
        ("/recall", &["post"], "Recall memories", "Memory"),
        ("/list", &["get"], "List memories", "Memory"),
        ("/tags", &["get"], "List memory tags", "Memory"),
        (
            "/tags/search",
            &["post"],
            "Search memories by tags",
            "Memory",
        ),
        ("/profile", &["get"], "Get user profile", "Memory"),
        (
            "/profile/synthesize",
            &["post"],
            "Synthesize user profile",
            "Memory",
        ),
        ("/me/stats", &["get"], "User-scoped stats", "Memory"),
        ("/links/{id}", &["get"], "List memory links", "Memory"),
        ("/versions/{id}", &["get"], "List memory versions", "Memory"),
        (
            "/memory/{id}",
            &["get", "delete"],
            "Read or delete memory",
            "Memory",
        ),
        ("/memory/{id}/update", &["post"], "Update memory", "Memory"),
        (
            "/memory/{id}/tags",
            &["put"],
            "Replace memory tags",
            "Memory",
        ),
        ("/memory/{id}/forget", &["post"], "Forget memory", "Memory"),
        (
            "/memory/{id}/archive",
            &["post"],
            "Archive memory",
            "Memory",
        ),
        (
            "/memory/{id}/unarchive",
            &["post"],
            "Unarchive memory",
            "Memory",
        ),
        ("/bootstrap", &["post"], "Bootstrap admin user", "Admin"),
        ("/keys", &["get", "post"], "Manage keys", "Auth"),
        ("/keys/{id}", &["delete"], "Revoke key", "Auth"),
        ("/keys/rotate", &["post"], "Rotate key", "Auth"),
        ("/users", &["get", "post"], "Manage users", "Auth"),
        ("/spaces", &["get", "post"], "Manage spaces", "Auth"),
        ("/spaces/{id}", &["delete"], "Delete space", "Auth"),
        ("/stats", &["get"], "Admin stats", "Admin"),
        ("/tasks", &["get", "post"], "Manage tasks", "Tasks"),
        ("/tasks/stats", &["get"], "Task stats", "Tasks"),
        (
            "/tasks/{id}",
            &["get", "patch", "delete"],
            "Read or modify task",
            "Tasks",
        ),
        ("/feed", &["get"], "Task feed", "Tasks"),
        ("/axon/publish", &["post"], "Publish event", "Axon"),
        ("/axon/events", &["get"], "List events", "Axon"),
        ("/axon/events/{id}", &["get"], "Get event", "Axon"),
        ("/axon/channels", &["get"], "List channels", "Axon"),
        ("/axon/stats", &["get"], "Axon stats", "Axon"),
        (
            "/broca/actions",
            &["get", "post"],
            "Manage Broca actions",
            "Broca",
        ),
        ("/broca/actions/{id}", &["get"], "Get Broca action", "Broca"),
        ("/broca/feed", &["get"], "Broca feed", "Broca"),
        ("/broca/stats", &["get"], "Broca stats", "Broca"),
        (
            "/soma/agents",
            &["get", "post"],
            "Manage Soma agents",
            "Soma",
        ),
        (
            "/soma/agents/{id}",
            &["get", "patch", "delete"],
            "Read or modify Soma agent",
            "Soma",
        ),
        (
            "/soma/agents/{id}/heartbeat",
            &["post"],
            "Record Soma heartbeat",
            "Soma",
        ),
        ("/soma/stats", &["get"], "Soma stats", "Soma"),
        ("/thymus/goals", &["get", "post"], "Manage goals", "Thymus"),
        (
            "/thymus/goals/{id}",
            &["get", "patch", "delete"],
            "Read or modify goal",
            "Thymus",
        ),
        ("/thymus/evaluate", &["post"], "Evaluate goal", "Thymus"),
        (
            "/thymus/evaluations",
            &["get"],
            "List evaluations",
            "Thymus",
        ),
        (
            "/thymus/evaluations/{id}",
            &["get"],
            "Get evaluation",
            "Thymus",
        ),
        (
            "/thymus/metrics",
            &["get", "post"],
            "Manage metrics",
            "Thymus",
        ),
        (
            "/thymus/metrics/summary",
            &["get"],
            "Metric summary",
            "Thymus",
        ),
        (
            "/thymus/initiatives",
            &["get", "post"],
            "Manage initiatives",
            "Thymus",
        ),
        (
            "/thymus/initiatives/{id}",
            &["get", "patch", "delete"],
            "Read or modify initiative",
            "Thymus",
        ),
        ("/thymus/stats", &["get"], "Thymus stats", "Thymus"),
        (
            "/loom/templates",
            &["get", "post"],
            "Manage loom templates",
            "Loom",
        ),
        (
            "/loom/templates/{id}",
            &["get", "patch", "delete"],
            "Read or modify loom template",
            "Loom",
        ),
        ("/loom/runs", &["get", "post"], "Manage loom runs", "Loom"),
        ("/loom/runs/{id}", &["get"], "Get loom run", "Loom"),
        (
            "/loom/runs/{id}/cancel",
            &["post"],
            "Cancel loom run",
            "Loom",
        ),
        ("/loom/runs/{id}/steps", &["get"], "List loom steps", "Loom"),
        ("/loom/runs/{id}/logs", &["get"], "List loom logs", "Loom"),
        (
            "/loom/steps/{id}/complete",
            &["post"],
            "Complete loom step",
            "Loom",
        ),
        ("/loom/steps/{id}/fail", &["post"], "Fail loom step", "Loom"),
        ("/loom/stats", &["get"], "Loom stats", "Loom"),
        ("/episodes", &["get", "post"], "Manage episodes", "Episodes"),
        (
            "/episodes/{id}",
            &["get", "patch"],
            "Read or update episode",
            "Episodes",
        ),
        (
            "/episodes/{id}/memories",
            &["post"],
            "Assign memories to episode",
            "Episodes",
        ),
        (
            "/episodes/{id}/finalize",
            &["post"],
            "Finalize episode",
            "Episodes",
        ),
        (
            "/conversations",
            &["get", "post"],
            "Manage conversations",
            "Conversations",
        ),
        (
            "/conversations/{id}",
            &["get", "patch", "delete"],
            "Read or modify conversation",
            "Conversations",
        ),
        (
            "/conversations/{id}/messages",
            &["get", "post"],
            "Manage conversation messages",
            "Conversations",
        ),
        (
            "/conversations/bulk",
            &["post"],
            "Bulk insert conversations",
            "Conversations",
        ),
        (
            "/conversations/upsert",
            &["post"],
            "Upsert conversation",
            "Conversations",
        ),
        (
            "/messages/search",
            &["post"],
            "Search messages",
            "Conversations",
        ),
        ("/entities", &["get", "post"], "Manage entities", "Graph"),
        (
            "/entities/{id}",
            &["get", "delete"],
            "Read or delete entity",
            "Graph",
        ),
        (
            "/entities/{id}/relationships",
            &["get"],
            "List entity relationships",
            "Graph",
        ),
        (
            "/entities/{id}/memories",
            &["get"],
            "List entity memories",
            "Graph",
        ),
        (
            "/entities/{id}/cooccurrences",
            &["get"],
            "List entity cooccurrences",
            "Graph",
        ),
        (
            "/entity-relationships",
            &["post"],
            "Create entity relationship",
            "Graph",
        ),
        ("/graph/build", &["post"], "Build graph", "Graph"),
        ("/graph/search", &["post"], "Search graph", "Graph"),
        (
            "/graph/neighborhood/{id}",
            &["get"],
            "Graph neighborhood",
            "Graph",
        ),
        (
            "/graph/communities",
            &["post"],
            "Detect communities",
            "Graph",
        ),
        (
            "/graph/communities/{id}/members",
            &["get"],
            "Community members",
            "Graph",
        ),
        (
            "/graph/communities/stats",
            &["get"],
            "Community stats",
            "Graph",
        ),
        ("/graph/pagerank", &["post"], "Run pagerank", "Graph"),
        (
            "/graph/cooccurrences/rebuild",
            &["post"],
            "Rebuild cooccurrences",
            "Graph",
        ),
        (
            "/memory/{id}/entities",
            &["get"],
            "Memory entities",
            "Graph",
        ),
        (
            "/intelligence/consolidate",
            &["post"],
            "Run consolidation",
            "Intelligence",
        ),
        (
            "/intelligence/reconsolidate",
            &["post"],
            "Run reconsolidation",
            "Intelligence",
        ),
        (
            "/intelligence/contradictions",
            &["post"],
            "Detect contradictions",
            "Intelligence",
        ),
        (
            "/intelligence/contradictions/list",
            &["get"],
            "List contradictions",
            "Intelligence",
        ),
        (
            "/intelligence/temporal/analyze",
            &["post"],
            "Analyze temporal data",
            "Intelligence",
        ),
        (
            "/intelligence/temporal/patterns",
            &["get"],
            "List temporal patterns",
            "Intelligence",
        ),
        (
            "/intelligence/digests",
            &["get"],
            "List digests",
            "Intelligence",
        ),
        (
            "/intelligence/digests/generate",
            &["post"],
            "Generate digest",
            "Intelligence",
        ),
        (
            "/intelligence/causal/chains",
            &["get", "post"],
            "Manage causal chains",
            "Intelligence",
        ),
        (
            "/intelligence/causal/chains/{id}",
            &["get"],
            "Get causal chain",
            "Intelligence",
        ),
        (
            "/intelligence/causal/links",
            &["post"],
            "Add causal link",
            "Intelligence",
        ),
        ("/skills", &["get", "post"], "Manage skills", "Skills"),
        ("/skills/search", &["post"], "Search skills", "Skills"),
        (
            "/skills/{id}",
            &["get", "delete"],
            "Read or delete skill",
            "Skills",
        ),
        ("/skills/{id}/update", &["post"], "Update skill", "Skills"),
        (
            "/skills/{id}/execute",
            &["post"],
            "Record skill execution",
            "Skills",
        ),
        (
            "/skills/{id}/executions",
            &["get"],
            "List skill executions",
            "Skills",
        ),
        ("/skills/{id}/judge", &["post"], "Judge skill", "Skills"),
        (
            "/skills/{id}/judgments",
            &["get"],
            "List skill judgments",
            "Skills",
        ),
        ("/skills/{id}/tags", &["get"], "List skill tags", "Skills"),
        (
            "/skills/{id}/deps",
            &["get"],
            "List skill dependencies",
            "Skills",
        ),
        (
            "/skills/{id}/lineage",
            &["get"],
            "Get skill lineage",
            "Skills",
        ),
        ("/tools/quality", &["post"], "Record tool quality", "Skills"),
        (
            "/tools/quality/{tool_name}",
            &["get"],
            "Get tool quality",
            "Skills",
        ),
        (
            "/skills/dashboard/health",
            &["get"],
            "Skills dashboard health",
            "Skills",
        ),
        (
            "/skills/dashboard/overview",
            &["get"],
            "Skills dashboard overview",
            "Skills",
        ),
        (
            "/skills/dashboard/stats",
            &["get"],
            "Skills dashboard stats",
            "Skills",
        ),
        (
            "/skills/{id}/detail",
            &["get"],
            "Get skill detail",
            "Skills",
        ),
        ("/skills/evolve", &["post"], "Evolve skills", "Skills"),
        ("/skills/{id}/fix", &["post"], "Fix skill", "Skills"),
        ("/skills/derive", &["post"], "Derive skill", "Skills"),
        ("/skills/capture", &["post"], "Capture skill", "Skills"),
        (
            "/skills/usage-stats",
            &["get"],
            "Skill usage stats",
            "Skills",
        ),
        (
            "/skills/cloud/search",
            &["post"],
            "Search cloud skills",
            "Skills",
        ),
        (
            "/skills/cloud/upload",
            &["post"],
            "Upload cloud skill",
            "Skills",
        ),
        (
            "/personality/detect",
            &["post"],
            "Detect personality",
            "Personality",
        ),
        (
            "/personality/traits",
            &["get", "post"],
            "Manage personality traits",
            "Personality",
        ),
        (
            "/personality/profile",
            &["get"],
            "Get personality profile",
            "Personality",
        ),
        (
            "/personality/profile/update",
            &["post"],
            "Update personality profile",
            "Personality",
        ),
        ("/sync/changes", &["get"], "Get sync changes", "Platform"),
        ("/audit", &["get"], "Audit log", "Security"),
        ("/quota", &["get"], "Quota status", "Security"),
        ("/usage", &["post"], "Record usage", "Security"),
        (
            "/rate-limit/{key}",
            &["get"],
            "Rate limit status",
            "Security",
        ),
        ("/activity", &["post"], "Report activity", "Activity"),
        ("/gate/check", &["post"], "Gate check", "Gate"),
        ("/gate/respond", &["post"], "Gate respond", "Gate"),
        ("/gate/complete", &["post"], "Gate complete", "Gate"),
        ("/growth/reflect", &["post"], "Growth reflection", "Growth"),
        (
            "/growth/observations",
            &["get"],
            "Growth observations",
            "Growth",
        ),
        (
            "/growth/materialize",
            &["post"],
            "Growth materialize",
            "Growth",
        ),
        ("/sessions", &["get", "post"], "Manage sessions", "Sessions"),
        ("/sessions/{id}", &["get"], "Get session", "Sessions"),
        (
            "/sessions/{id}/append",
            &["post"],
            "Append session output",
            "Sessions",
        ),
        (
            "/sessions/{id}/stream",
            &["get"],
            "Stream session output",
            "Sessions",
        ),
        ("/agents", &["get", "post"], "Manage agents", "Agents"),
        ("/agents/{id}", &["get"], "Get agent", "Agents"),
        ("/agents/{id}/revoke", &["post"], "Revoke agent", "Agents"),
        (
            "/agents/{id}/passport",
            &["get"],
            "Issue passport",
            "Agents",
        ),
        (
            "/agents/{id}/link-key",
            &["post"],
            "Link API key to agent",
            "Agents",
        ),
        (
            "/agents/{id}/executions",
            &["get"],
            "List agent executions",
            "Agents",
        ),
        (
            "/verify",
            &["post"],
            "Verify signed agent payload",
            "Agents",
        ),
        ("/artifacts/stats", &["get"], "Artifact stats", "Artifacts"),
        (
            "/artifacts/{memory_id}",
            &["get", "post"],
            "List or upload memory artifacts",
            "Artifacts",
        ),
        ("/artifact/{id}", &["get"], "Download artifact", "Artifacts"),
        ("/fsrs/review", &["post"], "Process FSRS review", "FSRS"),
        ("/fsrs/state", &["get"], "Get FSRS state", "FSRS"),
        ("/fsrs/init", &["post"], "Backfill FSRS state", "FSRS"),
        (
            "/grounding/sessions",
            &["get", "post"],
            "Manage grounding sessions",
            "Grounding",
        ),
        (
            "/grounding/sessions/{id}",
            &["get", "delete"],
            "Read or destroy grounding session",
            "Grounding",
        ),
        (
            "/grounding/tools",
            &["get"],
            "List grounding tools",
            "Grounding",
        ),
        (
            "/grounding/execute",
            &["post"],
            "Execute grounding tool",
            "Grounding",
        ),
        (
            "/grounding/quality",
            &["get"],
            "Grounding quality records",
            "Grounding",
        ),
        (
            "/grounding/providers",
            &["get"],
            "List grounding providers",
            "Grounding",
        ),
        (
            "/decay/refresh",
            &["post"],
            "Refresh decay scores",
            "Search",
        ),
        ("/decay/scores", &["get"], "List decay scores", "Search"),
        ("/onboard", &["post"], "Onboard", "Onboard"),
        ("/fetch", &["post"], "Fetch URL", "Onboard"),
        ("/import/bulk", &["post"], "Bulk import", "Ingestion"),
        ("/import/json", &["post"], "Import JSON", "Ingestion"),
        ("/import/mem0", &["post"], "Import mem0", "Ingestion"),
        (
            "/import/supermemory",
            &["post"],
            "Import supermemory",
            "Ingestion",
        ),
        ("/ingest", &["post"], "Ingest text", "Ingestion"),
        (
            "/ingest/stream",
            &["post"],
            "Ingest text with SSE progress events",
            "Ingestion",
        ),
        ("/add", &["post"], "Add conversation", "Ingestion"),
        ("/derive", &["post"], "Derive memories", "Ingestion"),
        ("/prompt", &["get"], "Get prompt", "Prompts"),
        ("/prompt/generate", &["post"], "Generate prompt", "Prompts"),
        ("/header", &["post"], "Post header", "Prompts"),
        (
            "/scratch",
            &["get", "put"],
            "Manage scratchpad",
            "Scratchpad",
        ),
        (
            "/scratch/{session}",
            &["delete"],
            "Delete scratch session",
            "Scratchpad",
        ),
        (
            "/scratch/{session}/{key}",
            &["delete"],
            "Delete scratch key",
            "Scratchpad",
        ),
        (
            "/scratch/{session}/promote",
            &["post"],
            "Promote scratch entry",
            "Scratchpad",
        ),
        ("/inbox", &["get"], "List inbox", "Inbox"),
        (
            "/inbox/{id}/approve",
            &["post"],
            "Approve inbox item",
            "Inbox",
        ),
        (
            "/inbox/{id}/reject",
            &["post"],
            "Reject inbox item",
            "Inbox",
        ),
        ("/inbox/{id}/edit", &["post"], "Edit inbox item", "Inbox"),
        ("/inbox/bulk", &["post"], "Bulk inbox action", "Inbox"),
        ("/pending", &["get"], "Legacy pending inbox list", "Inbox"),
        ("/pack", &["post"], "Pack memories", "Pack"),
        ("/projects", &["get", "post"], "Manage projects", "Projects"),
        (
            "/projects/{id}",
            &["get", "put", "delete"],
            "Read or modify project",
            "Projects",
        ),
        (
            "/projects/{id}/memories/{mid}",
            &["put", "delete"],
            "Link or unlink project memory",
            "Projects",
        ),
        ("/webhooks", &["get", "post"], "Manage webhooks", "Webhooks"),
        (
            "/webhooks/{id}",
            &["get", "put", "delete"],
            "Read or modify webhook",
            "Webhooks",
        ),
        ("/webhooks/test/{id}", &["post"], "Test webhook", "Webhooks"),
        ("/brain/stats", &["get"], "Brain stats", "Brain"),
        ("/brain/query", &["post"], "Brain query", "Brain"),
        ("/brain/absorb", &["post"], "Brain absorb", "Brain"),
        ("/brain/dream", &["post"], "Brain dream", "Brain"),
        ("/brain/feedback", &["post"], "Brain feedback", "Brain"),
        ("/brain/decay", &["post"], "Brain decay", "Brain"),
        ("/context", &["post"], "Build context", "Context"),
        ("/batch", &["post"], "Batch operations", "Batch"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_has_required_top_level_fields() {
        let spec = build_openapi_spec();
        assert_eq!(spec["openapi"], "3.1.0");
        assert!(spec["info"]["title"].is_string());
        assert!(spec["paths"].is_object());
        assert!(spec["components"]["securitySchemes"]["BearerAuth"].is_object());
        assert!(spec["components"]["schemas"]["Memory"].is_object());
        assert!(spec["components"]["schemas"]["StoreRequest"].is_object());
    }

    #[test]
    fn docs_route_present_in_spec() {
        let spec = build_openapi_spec();
        assert!(spec["paths"]["/docs"].is_object());
        assert!(spec["paths"]["/docs"]["get"].is_object());
    }

    #[test]
    fn store_route_has_request_body_and_typed_response() {
        let spec = build_openapi_spec();
        let op = &spec["paths"]["/store"]["post"];
        assert_eq!(
            op["requestBody"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/StoreRequest"
        );
        assert_eq!(
            op["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/StoreResponse"
        );
    }

    #[test]
    fn path_params_are_extracted() {
        let spec = build_openapi_spec();
        let op = &spec["paths"]["/memory/{id}"]["get"];
        let params = op["parameters"].as_array().expect("parameters array");
        assert!(params
            .iter()
            .any(|p| p["name"] == "id" && p["in"] == "path"));
    }

    #[test]
    fn list_route_has_pagination_params() {
        let spec = build_openapi_spec();
        let op = &spec["paths"]["/conversations"]["get"];
        let params = op["parameters"].as_array().expect("parameters array");
        let has_limit = params.iter().any(|p| {
            p.get("$ref").and_then(|v| v.as_str()) == Some("#/components/parameters/Limit")
        });
        assert!(has_limit);
    }

    #[test]
    fn security_requirement_applied_globally() {
        let spec = build_openapi_spec();
        let sec = spec["security"].as_array().expect("security array");
        assert_eq!(sec[0]["BearerAuth"], json!([]));
    }
}
