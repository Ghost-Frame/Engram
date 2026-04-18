use serde::{Deserialize, Serialize};

/// Per-stage summary returned by each dream-cycle stage function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageReport {
    pub stage: String,
    pub items_processed: usize,
    pub items_changed: usize,
    pub duration_ms: u64,
}

/// Combined result of a full dream cycle run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamCycleResult {
    pub user_id: i64,
    pub run_id: i64,
    pub stages: Vec<StageReport>,
    pub total_duration_ms: u64,
}
