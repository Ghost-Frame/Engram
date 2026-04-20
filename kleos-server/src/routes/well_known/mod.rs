use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::json;

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card))
        .route("/.well-known/agent-commerce.json", get(agent_commerce))
        .route("/llms.txt", get(llms_txt))
}

async fn agent_card() -> impl IntoResponse {
    let card = json!({
        "name": "kleos",
        "description": "Cognitive memory service for AI agents. Hybrid search, knowledge graph, spaced repetition, coordination services, quality tracking.",
        "url": "https://kleos.zanverse.com",
        "version": "1.0.0",
        "protocol": "a2a",
        "capabilities": {
            "streaming": false,
            "pushNotifications": true,
            "stateTransitionHistory": false
        },
        "skills": [
            {
                "id": "memory-search",
                "name": "Memory Search",
                "description": "4-channel hybrid search across vector, full-text, personality, and graph indexes. Question-type routing, FSRS decay, PageRank boost.",
                "inputModes": ["text"],
                "outputModes": ["text"]
            },
            {
                "id": "memory-store",
                "name": "Memory Storage",
                "description": "Store memories with automatic embedding, entity extraction, auto-linking, and FSRS initialization.",
                "inputModes": ["text"],
                "outputModes": ["text"]
            },
            {
                "id": "context-assembly",
                "name": "RAG Context Assembly",
                "description": "Budget-aware context window assembly from multiple memory sources.",
                "inputModes": ["text"],
                "outputModes": ["text"]
            },
            {
                "id": "knowledge-graph",
                "name": "Knowledge Graph",
                "description": "Entity relationships, community detection, PageRank, neighborhood traversal, structural analysis.",
                "inputModes": ["text"],
                "outputModes": ["text"]
            },
            {
                "id": "agent-coordination",
                "name": "Agent Coordination",
                "description": "Event bus (Axon), task tracking (Chiasm), agent registry (Soma), workflow orchestration (Loom), action ledger (Broca), quality evaluation (Thymus).",
                "inputModes": ["text"],
                "outputModes": ["text"]
            },
            {
                "id": "intelligence",
                "name": "Memory Intelligence",
                "description": "LLM-powered consolidation, contradiction detection, reflection, fact extraction, temporal analysis.",
                "inputModes": ["text"],
                "outputModes": ["text"]
            }
        ],
        "endpoints": {
            "search": "/search",
            "store": "/store",
            "recall": "/recall",
            "activity": "/activity",
            "discovery": "/.well-known/agent-commerce.json"
        },
        "authentication": {
            "type": "bearer",
            "header": "Authorization",
            "prefix": "Bearer",
            "key_prefix": "eg_",
            "alternative": "x402"
        },
        "links": {
            "openapi": "/openapi.json",
            "docs": "/docs"
        }
    });

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        Json(card),
    )
}

async fn agent_commerce(State(state): State<AppState>) -> impl IntoResponse {
    // Build service list from pricing table if available, otherwise static.
    let services = build_service_descriptors(&state).await;

    let descriptor = json!({
        "kleos": "1.0.0",
        "acp": "0.1.0",
        "provider": {
            "name": "Kleos",
            "organization": "Zanverse",
            "url": "https://kleos.zanverse.com"
        },
        "services": services,
        "attestation": {
            "registered_since": "2026-04-20T00:00:00Z",
            "verified_by": "self"
        },
        "registry": {
            "openapi": "/openapi.json",
            "a2a_agent_card": "/.well-known/agent-card.json",
            "mcp": {
                "transport": "stdio",
                "binary": "kleos-mcp",
                "tools": 57
            }
        }
    });

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=60"),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        Json(descriptor),
    )
}

async fn build_service_descriptors(state: &AppState) -> serde_json::Value {
    // Try to read from pricing table; fall back to static definitions.
    let pricing = kleos_lib::commerce::pricing::list_service_pricing(&state.db).await;

    if let Ok(prices) = pricing {
        if !prices.is_empty() {
            let services: Vec<serde_json::Value> = prices
                .iter()
                .map(|p| {
                    json!({
                        "id": p.service_id,
                        "pricing": {
                            "model": "per-call",
                            "amount": p.base_amount.to_string(),
                            "currency": p.currency,
                            "chain": p.chain,
                            "chain_id": p.chain_id
                        },
                        "invoke": {
                            "authentication": ["x402", "bearer"]
                        },
                        "annotations": {
                            "read_only": p.service_id.contains("search") || p.service_id.contains("recall"),
                            "idempotent": p.service_id.contains("search") || p.service_id.contains("recall")
                        }
                    })
                })
                .collect();
            return serde_json::Value::Array(services);
        }
    }

    // Static fallback with default pricing.
    json!([
        {
            "id": "kleos-search",
            "name": "Hybrid Memory Search",
            "description": "4-channel hybrid search: vector similarity, FTS5, personality signals, knowledge graph.",
            "pricing": { "model": "per-call", "amount": "0.005", "currency": "USDC", "chain": "base", "chain_id": 8453 },
            "invoke": { "url": "/search", "method": "POST", "authentication": ["x402", "bearer"] },
            "annotations": { "read_only": true, "idempotent": true }
        },
        {
            "id": "kleos-store",
            "name": "Memory Storage",
            "description": "Store a memory with automatic embedding, FTS indexing, FSRS initialization, entity extraction, auto-linking.",
            "pricing": { "model": "per-call", "amount": "0.01", "currency": "USDC", "chain": "base", "chain_id": 8453 },
            "invoke": { "url": "/store", "method": "POST", "authentication": ["x402", "bearer"] },
            "annotations": { "read_only": false, "idempotent": false }
        },
        {
            "id": "kleos-recall",
            "name": "RAG Context Assembly",
            "description": "Budget-aware context assembly for RAG.",
            "pricing": { "model": "per-call", "amount": "0.01", "currency": "USDC", "chain": "base", "chain_id": 8453 },
            "invoke": { "url": "/recall", "method": "POST", "authentication": ["x402", "bearer"] },
            "annotations": { "read_only": true, "idempotent": true }
        },
        {
            "id": "kleos-intelligence",
            "name": "Memory Intelligence",
            "description": "LLM-powered memory operations: consolidation, contradiction detection, reflection, fact extraction.",
            "pricing": { "model": "per-call", "amount": "0.05", "currency": "USDC", "chain": "base", "chain_id": 8453 },
            "invoke": { "url": "/intelligence/extract", "method": "POST", "authentication": ["x402", "bearer"] },
            "annotations": { "read_only": false, "idempotent": false }
        },
        {
            "id": "kleos-activity",
            "name": "Activity Fan-out",
            "description": "Single-call fan-out to 6 subsystems.",
            "pricing": { "model": "per-call", "amount": "0.005", "currency": "USDC", "chain": "base", "chain_id": 8453 },
            "invoke": { "url": "/activity", "method": "POST", "authentication": ["x402", "bearer"] },
            "annotations": { "read_only": false, "idempotent": false }
        }
    ])
}

async fn llms_txt() -> Response {
    let text = r#"# Kleos

Cognitive memory service for AI agents by Zanverse.

## What it does

Stores, searches, and connects memories for AI agents. 4-channel hybrid search (vector + full-text + personality + knowledge graph). Spaced repetition decay. 6 coordination subsystems for multi-agent workflows.

## API

- POST /search -- hybrid memory search
- POST /store -- store a memory
- POST /recall -- RAG context assembly
- POST /activity -- fan-out to all subsystems
- POST /context -- budget-aware context window

Full OpenAPI spec: /openapi.json
Service descriptor: /.well-known/agent-commerce.json

## Auth

Bearer token (API key) or x402 pay-per-call (USDC on Base L2).

## MCP

57 tools via stdio transport. Binary: kleos-mcp
"#;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(axum::body::Body::from(text))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}
