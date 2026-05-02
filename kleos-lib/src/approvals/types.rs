use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

impl ApprovalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "approved" => Some(Self::Approved),
            "denied" => Some(Self::Denied),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub action: String,
    pub context: Option<String>,
    pub requester: String,
    pub status: ApprovalStatus,
    pub decision_by: Option<String>,
    pub decision_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub user_id: i64,
}

impl Approval {
    /// Seconds remaining until expiry. Returns 0 if already expired.
    pub fn seconds_remaining(&self) -> i64 {
        let now = Utc::now();
        if now >= self.expires_at {
            0
        } else {
            (self.expires_at - now).num_seconds()
        }
    }

    /// Check if the approval has expired.
    pub fn is_expired(&self) -> bool {
        self.status == ApprovalStatus::Pending && Utc::now() >= self.expires_at
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateApprovalRequest {
    pub action: String,
    pub context: Option<String>,
    pub requester: String,
    /// Override the default 120s window.
    #[serde(default)]
    pub window_secs: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalDecision {
    Approved,
    Denied,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecideRequest {
    pub decision: ApprovalDecision,
    pub decided_by: Option<String>,
    pub reason: Option<String>,
}
