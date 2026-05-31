use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};

use crate::state::AppState;

/// Public URL where this Kleos instance is reachable. Operators set
/// `KLEOS_PUBLIC_URL` per deployment; the default is a neutral
/// example-domain placeholder so a fresh checkout has no operator
/// branding baked into the binary.
fn public_url() -> String {
    std::env::var("KLEOS_PUBLIC_URL").unwrap_or_else(|_| "https://kleos.example.com".into())
}

/// Display name of the operating organization. Operators set
/// `KLEOS_ORG_NAME` per deployment; default is the neutral product name.
fn org_name() -> String {
    std::env::var("KLEOS_ORG_NAME").unwrap_or_else(|_| "Kleos".into())
}

/// Count the currently advertised curated MCP tools.
fn curated_mcp_tool_count() -> usize {
    kleos_mcp::tools::registry().len()
}

/// Build the MCP registry descriptor used by public discovery metadata.
fn mcp_registry_descriptor() -> Value {
    json!({
        "transport": "stdio",
        "binary": "kleos-mcp",
        "tools": curated_mcp_tool_count()
    })
}

/// Build the public well-known route table.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/agent-card.json", get(agent_card))
        .route("/.well-known/agent-commerce.json", get(agent_commerce))
        .route("/llms.txt", get(llms_txt))
}

/// Return the A2A-style agent card for this Kleos instance.
async fn agent_card() -> impl IntoResponse {
    let url = public_url();
    let card = json!({
        "name": "kleos",
        "description": "Cognitive memory service for AI agents. Hybrid search, knowledge graph, spaced repetition, coordination services, quality tracking.",
        "url": url,
        "version": env!("CARGO_PKG_VERSION"),
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
            "key_prefix": "kleos_",
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
        ],
        Json(card),
    )
}

/// Return commerce and machine-discovery metadata for agent consumers.
async fn agent_commerce(State(state): State<AppState>) -> impl IntoResponse {
    // Build service list from pricing table if available, otherwise static.
    let services = build_service_descriptors(&state).await;
    let url = public_url();
    let org = org_name();

    let descriptor = json!({
        "kleos": env!("CARGO_PKG_VERSION"),
        "acp": "0.1.0",
        "provider": {
            "name": "Kleos",
            "organization": org,
            "url": url
        },
        "services": services,
        "attestation": {
            "registered_since": "2026-04-20T00:00:00Z",
            "verified_by": "self"
        },
        "registry": {
            "openapi": "/openapi.json",
            "a2a_agent_card": "/.well-known/agent-card.json",
            "mcp": mcp_registry_descriptor()
        }
    });

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        Json(descriptor),
    )
}

/// Build service descriptors from pricing rows, falling back to static defaults.
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

/// Return a compact `llms.txt` discovery document.
async fn llms_txt() -> Response {
    let org = org_name();
    let tools = curated_mcp_tool_count();
    let text = format!(
        r#"# Kleos

Cognitive memory service for AI agents by {org}.

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

{tools} tools via stdio transport. Binary: kleos-mcp
"#
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .body(axum::body::Body::from(text))
        .unwrap_or_else(|_| Response::new(axum::body::Body::empty()))
}

#[cfg(test)]
/// Unit tests for public well-known discovery metadata.
mod tests {
    use super::*;

    /// Verifies that commerce metadata advertises the live curated MCP registry size.
    #[test]
    fn mcp_registry_descriptor_uses_curated_count() {
        let descriptor = mcp_registry_descriptor();
        assert_eq!(descriptor["transport"], "stdio");
        assert_eq!(descriptor["binary"], "kleos-mcp");
        assert_eq!(
            descriptor["tools"].as_u64(),
            Some(curated_mcp_tool_count() as u64),
            "commerce metadata should use the live curated registry count"
        );
    }

    /// Verifies that `llms.txt` advertises the live curated MCP registry size.
    #[tokio::test]
    async fn llms_txt_uses_curated_mcp_registry_count() {
        let response = llms_txt().await;
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("llms.txt body should be readable");
        let body = String::from_utf8(bytes.to_vec()).expect("llms.txt should be utf-8");
        let expected = format!(
            "{} tools via stdio transport",
            kleos_mcp::tools::registry().len()
        );

        assert!(
            body.contains(&expected),
            "llms.txt should contain live registry count {expected:?}, got {body:?}"
        );
    }
}
