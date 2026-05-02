use serde::{Deserialize, Serialize};

use kleos_lib::approvals::{Approval, ApprovalDecision};

#[derive(Debug, Serialize)]
pub(super) struct ApprovalResponse {
    #[serde(flatten)]
    pub approval: Approval,
    pub seconds_remaining: i64,
}

impl From<Approval> for ApprovalResponse {
    fn from(approval: Approval) -> Self {
        let seconds_remaining = approval.seconds_remaining();
        Self {
            approval,
            seconds_remaining,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct DecideBody {
    pub decision: ApprovalDecision,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
}
