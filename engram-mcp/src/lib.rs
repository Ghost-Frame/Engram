pub mod auth;
pub mod tools;
pub mod transport;

use engram_lib::config::Config;
use engram_lib::db::Database;
use engram_lib::{EngError, Result};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct App {
    pub db: Arc<Database>,
    pub config: Arc<Config>,
}

impl App {
    pub async fn from_env() -> Result<Self> {
        let config = Config::from_env();
        let db = Database::connect_with_config(&config).await?;
        Ok(Self {
            db: Arc::new(db),
            config: Arc::new(config),
        })
    }

    pub async fn for_tests() -> Result<Self> {
        Ok(Self {
            db: Arc::new(Database::connect_memory().await?),
            config: Arc::new(Config::default()),
        })
    }
}

fn protocol_version() -> &'static str {
    "2024-11-05"
}

fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }})
}

fn missing_id(req: &Value) -> Option<Value> {
    req.get("id").cloned()
}

pub async fn handle_jsonrpc(app: &App, req: Value) -> Option<Value> {
    let id = missing_id(&req);
    let method = req.get("method")?.as_str()?;

    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
    match method {
        "initialize" => {
            let id = id?;
            Some(response(
                id,
                json!({
                    "protocolVersion": protocol_version(),
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "engram-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            ))
        }
        "notifications/initialized" => None,
        "tools/list" => {
            let id = id?;
            Some(response(id, json!({ "tools": tools::registry() })))
        }
        "tools/call" => {
            let id = id?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let call_result = tools::dispatch(app, &name, arguments).await;
            match call_result {
                Ok(value) => Some(response(
                    id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                        }],
                        "structuredContent": value,
                        "isError": false
                    }),
                )),
                Err(err) => {
                    let message = err.to_string();
                    Some(response(
                        id,
                        json!({
                            "content": [{
                                "type": "text",
                                "text": message
                            }],
                            "isError": true
                        }),
                    ))
                }
            }
        }
        _ => id.map(|id| error_response(id, -32601, "method not found")),
    }
}

pub fn invalid_input(message: impl Into<String>) -> EngError {
    EngError::InvalidInput(message.into())
}

pub fn require_object(value: &Value) -> Result<&serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid_input("arguments must be a JSON object"))
}
