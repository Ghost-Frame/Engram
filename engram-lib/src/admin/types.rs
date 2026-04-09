//! Admin types -- ported from TS admin/types.ts

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactResult {
    pub size_before: i64,
    pub size_after: i64,
    pub saved_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcBreakdown {
    pub forgotten_memories: i64,
    pub expired_memories: i64,
    pub orphaned_embeddings: i64,
    pub old_audit_entries: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcResult {
    pub total_cleaned: i64,
    pub breakdown: GcBreakdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaResult {
    pub tables: Vec<SchemaTable>,
    pub indexes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaTable {
    pub name: String,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceStatus {
    pub enabled: bool,
    pub message: Option<String>,
    pub since: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaTargets {
    pub uptime_pct: f64,
    pub p99_latency_ms: i64,
    pub error_rate_pct: f64,
}

impl Default for SlaTargets {
    fn default() -> Self {
        Self { uptime_pct: 99.9, p99_latency_ms: 500, error_rate_pct: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaResult {
    pub targets: SlaTargets,
    pub current_uptime_pct: f64,
    pub current_error_rate_pct: f64,
    pub total_requests: i64,
    pub total_errors: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRow {
    pub user_id: i64,
    pub username: String,
    pub memory_count: i64,
    pub conversation_count: i64,
    pub api_key_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRow {
    pub id: i64,
    pub username: String,
    pub role: String,
    pub memory_count: i64,
    pub key_count: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisionResult {
    pub user_id: i64,
    pub username: String,
    pub api_key: String,
    pub space_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportData {
    pub users: Vec<serde_json::Value>,
    pub memories: Vec<serde_json::Value>,
    pub conversations: Vec<serde_json::Value>,
    pub api_keys: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupVerifyResult {
    pub integrity: String,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateRow {
    pub key: String,
    pub value: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct ProvisionBody {
    pub username: String,
    pub email: Option<String>,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MaintenanceBody {
    pub enabled: bool,
    pub message: Option<String>,
}
