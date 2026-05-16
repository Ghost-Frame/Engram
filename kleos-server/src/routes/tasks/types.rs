//! Request/response types for the Chiasm tasks HTTP routes.

use serde::Deserialize;

/// Query parameters for listing tasks.
#[derive(Debug, Deserialize)]
pub(super) struct ListTasksParams {
    /// Filter by agent name.
    pub agent: Option<String>,
    /// Filter by project name.
    pub project: Option<String>,
    /// Filter by task status.
    pub status: Option<String>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
    /// Number of results to skip for pagination.
    pub offset: Option<usize>,
}

/// HTTP request body for creating a Chiasm task.
#[derive(Debug, Deserialize)]
pub(super) struct CreateTaskBody {
    /// Agent to assign.
    pub agent: String,
    /// Project the task belongs to.
    pub project: String,
    /// Short title.
    pub title: String,
    /// Initial status.
    pub status: Option<String>,
    /// Optional description.
    pub summary: Option<String>,
    /// Description of expected output.
    pub expected_output: Option<String>,
    /// Format of expected output.
    pub output_format: Option<String>,
    /// Precondition for task start.
    pub condition: Option<String>,
    /// Guardrail validation URL.
    pub guardrail_url: Option<String>,
    /// Heartbeat interval in seconds.
    pub heartbeat_interval: Option<i64>,
}

/// HTTP request body for partially updating a Chiasm task.
#[derive(Debug, Deserialize)]
pub(super) struct UpdateTaskBody {
    /// New title, if changing.
    pub title: Option<String>,
    /// New summary, if changing.
    pub summary: Option<String>,
    /// New status, if changing.
    pub status: Option<String>,
    /// New agent assignment, if changing.
    pub agent: Option<String>,
}

/// Query parameters for task history requests.
#[derive(Debug, Deserialize)]
pub(super) struct HistoryParams {
    /// Maximum number of history entries to return.
    pub limit: Option<usize>,
}

/// HTTP request body for submitting task output.
#[derive(Debug, Deserialize)]
pub(super) struct SubmitOutputBody {
    /// The output content produced by the agent.
    pub output: String,
}

/// HTTP request body for submitting task feedback.
#[derive(Debug, Deserialize)]
pub(super) struct SubmitFeedbackBody {
    /// Feedback from reviewer or guardrail rejection.
    pub feedback: String,
}

/// HTTP request body for adding task dependencies.
#[derive(Debug, Deserialize)]
pub(super) struct AddDepsBody {
    /// List of task IDs this task depends on.
    pub depends_on: Vec<i64>,
}

/// HTTP request body for creating path claims.
#[derive(Debug, Deserialize)]
pub(super) struct CreateClaimsBody {
    /// Agent creating the claims.
    pub agent: String,
    /// Project the paths belong to.
    pub project: String,
    /// File paths to claim.
    pub paths: Vec<String>,
    /// TTL in seconds (defaults to 1800 = 30 minutes).
    pub ttl_seconds: Option<i64>,
}

/// HTTP request body for checking path conflicts.
#[derive(Debug, Deserialize)]
pub(super) struct CheckConflictsBody {
    /// Project to check in.
    pub project: String,
    /// Paths to check for conflicts.
    pub paths: Vec<String>,
    /// Task ID to exclude from conflict check (usually the requesting task).
    pub exclude_task_id: Option<i64>,
}

/// Query params for listing claims by project.
#[derive(Debug, Deserialize)]
pub(super) struct ClaimsProjectParams {
    /// Project to list claims for.
    pub project: String,
}

/// HTTP request body for enqueuing a new task into the work queue.
#[derive(Debug, Deserialize)]
pub(super) struct EnqueueBody {
    /// Project for the queued task.
    pub project: String,
    /// Short title.
    pub title: String,
    /// Optional description.
    pub summary: Option<String>,
}

/// HTTP request body for claiming the next available task from the queue.
#[derive(Debug, Deserialize)]
pub(super) struct ClaimBody {
    /// Agent claiming the task.
    pub agent: String,
    /// Optionally restrict to a specific project.
    pub project: Option<String>,
}
