//! Chiasm multi-agent task coordination service.
//!
//! Provides task lifecycle management, dependency tracking, path-based claims,
//! heartbeat monitoring, stale detection, and a work queue. Submodules split
//! core CRUD from coordination concerns.

/// Core task CRUD operations and lifecycle management.
mod tasks;

/// Task dependency DAG with circular detection and auto-unblock.
pub mod dependencies;

/// Path-based resource claims for multi-agent coordination.
pub mod claims;

/// Heartbeat tracking and stale-task detection.
pub mod heartbeat;

/// Work queue for unassigned tasks -- enqueue and claim-next.
pub mod queue;

/// Per-agent bearer keys (admin-managed). Mirrors the standalone chiasm
/// admin/keys surface; raw keys are returned exactly once on creation.
pub mod keys;

pub use tasks::*;

use crate::db::Database;

/// Fire-and-forget event emission for Chiasm state changes.
///
/// Publishes to the "tasks" channel via Axon's publish-and-fanout pipeline.
/// Errors are logged but never propagated -- event emission must not break
/// the primary operation.
pub(crate) async fn emit_chiasm_event(db: &Database, action: &str, payload: serde_json::Value) {
    let req = crate::services::axon::PublishEventRequest {
        channel: "tasks".to_string(),
        action: action.to_string(),
        payload: Some(payload),
        source: Some("chiasm".to_string()),
        agent: None,
        user_id: Some(1),
    };
    if let Err(e) = crate::services::axon::fanout::publish_and_fanout(db, req).await {
        tracing::warn!("chiasm event emission failed: {}", e);
    }
}
