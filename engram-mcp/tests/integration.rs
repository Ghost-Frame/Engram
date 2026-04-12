use engram_mcp::{handle_jsonrpc, App};
use serde_json::json;

async fn test_app_and_token() -> (App, String) {
    let app = App::for_tests().await.unwrap();
    app.db.conn.execute(
        "INSERT INTO users (username, email, role, is_admin) VALUES ('mcp-test', 'mcp@example.com', 'admin', 1)",
        (),
    ).await.unwrap();
    let (_, token) = engram_lib::auth::create_key(
        &app.db,
        1,
        "mcp-test",
        vec![
            engram_lib::auth::Scope::Read,
            engram_lib::auth::Scope::Write,
            engram_lib::auth::Scope::Admin,
        ],
        None,
    )
    .await
    .unwrap();
    (app, token)
}

async fn call(
    app: &App,
    token: &str,
    name: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let mut args = arguments.as_object().cloned().unwrap_or_default();
    args.insert("bearer_token".into(), json!(token));
    handle_jsonrpc(
        app,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": args,
            }
        }),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn initialize_and_list_tools() {
    let (app, _) = test_app_and_token().await;
    let init = handle_jsonrpc(
        &app,
        json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{}
        }),
    )
    .await
    .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "engram-mcp");

    let list = handle_jsonrpc(
        &app,
        json!({
            "jsonrpc":"2.0","id":2,"method":"tools/list","params":{}
        }),
    )
    .await
    .unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 30);
}

#[tokio::test]
async fn memory_round_trip_and_service_tools() {
    let (app, token) = test_app_and_token().await;

    let stored = call(
        &app,
        &token,
        "memory.store",
        json!({
            "content": "The MCP integration stores searchable test memories.",
            "category": "reference"
        }),
    )
    .await;
    assert_eq!(stored["result"]["isError"], false);

    let search = call(
        &app,
        &token,
        "memory.search",
        json!({
            "query": "searchable test memories",
            "limit": 5
        }),
    )
    .await;
    assert_eq!(search["result"]["isError"], false);

    let task = call(
        &app,
        &token,
        "services.chiasm_create_task",
        json!({
            "agent": "tester",
            "project": "engram-mcp",
            "title": "verify tool coverage"
        }),
    )
    .await;
    assert_eq!(task["result"]["isError"], false);

    let event = call(
        &app,
        &token,
        "services.axon_publish",
        json!({
            "channel": "tasks",
            "action": "task.started",
            "payload": {"id": 1}
        }),
    )
    .await;
    assert_eq!(event["result"]["isError"], false);
}

#[cfg(feature = "http")]
#[tokio::test]
async fn http_transport_round_trip() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::util::ServiceExt;

    let (app, token) = test_app_and_token().await;
    let router = engram_mcp::transport::http::router(app.clone());

    let init = router
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0",
                        "id":1,
                        "method":"initialize",
                        "params":{}
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(init.status(), StatusCode::OK);

    let store = router
        .clone()
        .oneshot(
            Request::post("/mcp")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "jsonrpc":"2.0",
                        "id":2,
                        "method":"tools/call",
                        "params":{
                            "name":"memory.store",
                            "arguments":{
                                "bearer_token": token,
                                "content":"HTTP MCP transport stores memories too.",
                                "category":"reference"
                            }
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(store.status(), StatusCode::OK);

    let body = axum::body::to_bytes(store.into_body(), usize::MAX)
        .await
        .unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["result"]["isError"], false);
}
