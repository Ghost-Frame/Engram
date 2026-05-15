//! Integration tests verifying that data written into one tenant shard is not
//! visible from a different tenant shard.
//!
//! Tenant isolation is enforced at the database level: each tenant gets its
//! own SQLite file under `data_dir/tenants/<tenant_id>/engram.db`. These tests
//! spin up two tenants in a temp directory, insert data into tenant_a, then
//! query tenant_b and assert the result is empty.
//!
//! These tests supersede the in-source `#[ignore]` tests in:
//!   - kleos-lib/src/approvals/mod.rs   (test_tenant_isolation)
//!   - kleos-lib/src/services/axon.rs   (consume_is_scoped_by_user)
//!   - kleos-lib/src/services/soma.rs   (list_is_scoped_by_user)
//!   - kleos-lib/src/services/chiasm.rs (list_is_scoped_by_user)

use std::sync::Arc;
use tempfile::tempdir;

use kleos_lib::tenant::{TenantConfig, TenantHandle, TenantRegistry};

/// Spin up two isolated tenant handles backed by a temporary directory.
async fn two_tenants() -> (Arc<TenantHandle>, Arc<TenantHandle>) {
    let dir = tempdir().expect("tempdir");
    let registry = TenantRegistry::new(
        dir.path(),
        TenantConfig::default(),
        128, // minimal vector dimensions for tests
        false,
        None,
    )
    .expect("registry");

    let tenant_a = registry.get_or_create("tenant_a").await.expect("tenant_a");
    let tenant_b = registry.get_or_create("tenant_b").await.expect("tenant_b");

    // Keep dir alive for the lifetime of the handles by leaking the tempdir.
    // The OS cleans up temp dirs on process exit anyway.
    std::mem::forget(dir);

    (tenant_a, tenant_b)
}

// ---------------------------------------------------------------------------
// Approvals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn approvals_isolated_across_tenants() {
    use kleos_lib::approvals::{create_approval, list_pending, CreateApprovalRequest};

    let (tenant_a, tenant_b) = two_tenants().await;
    let db_a = tenant_a.database();
    let db_b = tenant_b.database();

    let req = CreateApprovalRequest {
        action: "DELETE /memories/42".to_string(),
        context: Some(r#"{"memory_id": 42}"#.to_string()),
        requester: "test-agent".to_string(),
        window_secs: Some(300),
    };

    // Insert into tenant_a
    create_approval(&db_a, &req, 1)
        .await
        .expect("create in tenant_a");

    // tenant_b's pending list must be empty
    let pending_b = list_pending(&db_b, 1).await.expect("list_pending tenant_b");
    assert!(
        pending_b.is_empty(),
        "tenant_b should see no approvals, got {:?}",
        pending_b
    );
}

// ---------------------------------------------------------------------------
// Axon (events)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn axon_events_isolated_across_tenants() {
    use kleos_lib::services::axon::{consume, publish_event, PublishEventRequest};

    let (tenant_a, tenant_b) = two_tenants().await;
    let db_a = tenant_a.database();
    let db_b = tenant_b.database();

    // Publish an event into tenant_a
    publish_event(
        &db_a,
        PublishEventRequest {
            channel: "test-channel".into(),
            action: "ping".into(),
            payload: Some(serde_json::json!({"k": "v"})),
            source: Some("agent-a".into()),
            agent: None,
            user_id: Some(1),
        },
    )
    .await
    .expect("publish into tenant_a");

    // Consume from tenant_b on the same channel -- must be empty
    let events_b = consume(&db_b, "agent-x", "test-channel", 100, 1)
        .await
        .expect("consume from tenant_b");
    assert!(
        events_b.is_empty(),
        "tenant_b should see no events, got {:?}",
        events_b
    );
}

// ---------------------------------------------------------------------------
// Soma (agents)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn soma_agents_isolated_across_tenants() {
    use kleos_lib::services::soma::{list_agents, register_agent, RegisterAgentRequest};

    let (tenant_a, tenant_b) = two_tenants().await;
    let db_a = tenant_a.database();
    let db_b = tenant_b.database();

    // Register an agent in tenant_a
    register_agent(
        &db_a,
        RegisterAgentRequest {
            name: "agent-alpha".into(),
            type_: "llm".into(),
            description: Some("belongs to tenant_a".into()),
            capabilities: None,
            config: None,
            user_id: Some(1),
        },
    )
    .await
    .expect("register in tenant_a");

    // List agents in tenant_b -- must be empty
    let agents_b = list_agents(&db_b, 1, None, None, 100)
        .await
        .expect("list_agents tenant_b");
    assert!(
        agents_b.is_empty(),
        "tenant_b should see no agents, got {:?}",
        agents_b
    );
}

// ---------------------------------------------------------------------------
// Chiasm (tasks)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn chiasm_tasks_isolated_across_tenants() {
    use kleos_lib::services::chiasm::{create_task, list_tasks, CreateTaskRequest};

    let (tenant_a, tenant_b) = two_tenants().await;
    let db_a = tenant_a.database();
    let db_b = tenant_b.database();

    // Create a task in tenant_a
    create_task(
        &db_a,
        CreateTaskRequest {
            agent: "agent-alpha".into(),
            project: "project-x".into(),
            title: "tenant_a task".into(),
            status: Some("active".into()),
            summary: None,
            user_id: Some(1),
            expected_output: None,
            output_format: None,
            condition: None,
            guardrail_url: None,
            heartbeat_interval: None,
        },
    )
    .await
    .expect("create_task in tenant_a");

    // List tasks in tenant_b -- must be empty
    let tasks_b = list_tasks(&db_b, 1, None, None, None, 100, 0)
        .await
        .expect("list_tasks tenant_b");
    assert!(
        tasks_b.is_empty(),
        "tenant_b should see no tasks, got {:?}",
        tasks_b
    );
}
