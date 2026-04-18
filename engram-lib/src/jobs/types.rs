use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
}
impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
    pub fn from_str_loose(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: i64,
    pub job_type: String,
    pub payload: String,
    pub status: JobStatus,
    pub attempts: i32,
    pub max_attempts: i32,
    pub error: Option<String>,
    pub created_at: String,
    pub claimed_at: Option<String>,
    pub completed_at: Option<String>,
    pub next_retry_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JobStats {
    pub pending: i64,
    pub running: i64,
    pub completed: i64,
    pub failed: i64,
}
