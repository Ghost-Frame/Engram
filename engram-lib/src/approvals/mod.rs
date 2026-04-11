use crate::db::Database;
use crate::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Default approval window in seconds.
pub const DEFAULT_APPROVAL_WINDOW_SECS: i64 = 120;

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

/// Create a new approval request with a 120s (or custom) window.
pub async fn create_approval(
    db: &Database,
    req: &CreateApprovalRequest,
    user_id: i64,
) -> Result<Approval> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now();
    let window = req.window_secs.unwrap_or(DEFAULT_APPROVAL_WINDOW_SECS);
    let expires_at = now + Duration::seconds(window);

    let created_str = now.to_rfc3339();
    let expires_str = expires_at.to_rfc3339();

    db.conn
        .execute(
            "INSERT INTO approvals (id, action, context, requester, status, created_at, expires_at, user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            libsql::params![
                id.clone(),
                req.action.clone(),
                req.context.clone(),
                req.requester.clone(),
                "pending",
                created_str,
                expires_str,
                user_id,
            ],
        )
        .await?;

    Ok(Approval {
        id,
        action: req.action.clone(),
        context: req.context.clone(),
        requester: req.requester.clone(),
        status: ApprovalStatus::Pending,
        decision_by: None,
        decision_reason: None,
        created_at: now,
        expires_at,
        decided_at: None,
        user_id,
    })
}

/// Get a single approval by ID.
pub async fn get_approval(db: &Database, id: &str, user_id: i64) -> Result<Option<Approval>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, action, context, requester, status, decision_by, decision_reason,
                    created_at, expires_at, decided_at, user_id
             FROM approvals WHERE id = ?1 AND user_id = ?2",
            libsql::params![id, user_id],
        )
        .await?;

    match rows.next().await? {
        Some(row) => Ok(Some(row_to_approval(&row)?)),
        None => Ok(None),
    }
}

/// List all pending approvals for a user, ordered by expiry (soonest first).
pub async fn list_pending(db: &Database, user_id: i64) -> Result<Vec<Approval>> {
    let mut rows = db
        .conn
        .query(
            "SELECT id, action, context, requester, status, decision_by, decision_reason,
                    created_at, expires_at, decided_at, user_id
             FROM approvals
             WHERE user_id = ?1 AND status = 'pending'
             ORDER BY expires_at ASC",
            libsql::params![user_id],
        )
        .await?;

    let mut approvals = Vec::new();
    while let Some(row) = rows.next().await? {
        approvals.push(row_to_approval(&row)?);
    }
    Ok(approvals)
}

/// Decide on an approval (approve or deny).
pub async fn decide(
    db: &Database,
    id: &str,
    req: &DecideRequest,
    user_id: i64,
) -> Result<Approval> {
    // First check it exists and is pending
    let approval = get_approval(db, id, user_id)
        .await?
        .ok_or_else(|| crate::EngError::NotFound(format!("approval {} not found", id)))?;

    if approval.status != ApprovalStatus::Pending {
        return Err(crate::EngError::InvalidInput(format!(
            "approval {} is not pending (status: {:?})",
            id, approval.status
        )));
    }

    // Check if expired
    if approval.is_expired() {
        // Mark as expired in DB
        db.conn
            .execute(
                "UPDATE approvals SET status = 'expired' WHERE id = ?1 AND user_id = ?2",
                libsql::params![id, user_id],
            )
            .await?;
        return Err(crate::EngError::InvalidInput(format!(
            "approval {} has expired",
            id
        )));
    }

    let now = Utc::now();
    let decided_str = now.to_rfc3339();
    let new_status = match req.decision {
        ApprovalDecision::Approved => "approved",
        ApprovalDecision::Denied => "denied",
    };

    db.conn
        .execute(
            "UPDATE approvals
             SET status = ?1, decision_by = ?2, decision_reason = ?3, decided_at = ?4
             WHERE id = ?5 AND user_id = ?6",
            libsql::params![
                new_status,
                req.decided_by.clone(),
                req.reason.clone(),
                decided_str,
                id,
                user_id,
            ],
        )
        .await?;

    Ok(Approval {
        status: match req.decision {
            ApprovalDecision::Approved => ApprovalStatus::Approved,
            ApprovalDecision::Denied => ApprovalStatus::Denied,
        },
        decision_by: req.decided_by.clone(),
        decision_reason: req.reason.clone(),
        decided_at: Some(now),
        ..approval
    })
}

/// Expire all stale pending approvals. Returns the number of rows updated.
pub async fn expire_stale(db: &Database) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let rows = db
        .conn
        .execute(
            "UPDATE approvals SET status = 'expired'
             WHERE status = 'pending' AND expires_at < ?1",
            libsql::params![now],
        )
        .await?;
    Ok(rows)
}

/// Expire stale approvals for a specific user.
pub async fn expire_stale_for_user(db: &Database, user_id: i64) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let rows = db
        .conn
        .execute(
            "UPDATE approvals SET status = 'expired'
             WHERE status = 'pending' AND expires_at < ?1 AND user_id = ?2",
            libsql::params![now, user_id],
        )
        .await?;
    Ok(rows)
}

fn row_to_approval(row: &libsql::Row) -> Result<Approval> {
    let id: String = row.get(0)?;
    let action: String = row.get(1)?;
    let context: Option<String> = row.get(2)?;
    let requester: String = row.get(3)?;
    let status_str: String = row.get(4)?;
    let decision_by: Option<String> = row.get(5)?;
    let decision_reason: Option<String> = row.get(6)?;
    let created_at_str: String = row.get(7)?;
    let expires_at_str: String = row.get(8)?;
    let decided_at_str: Option<String> = row.get(9)?;
    let user_id: i64 = row.get(10)?;

    let status = ApprovalStatus::parse(&status_str)
        .ok_or_else(|| crate::EngError::Internal(format!("invalid status: {}", status_str)))?;

    let created_at = DateTime::parse_from_rfc3339(&created_at_str)
        .map_err(|e| crate::EngError::Internal(format!("invalid created_at: {}", e)))?
        .with_timezone(&Utc);

    let expires_at = DateTime::parse_from_rfc3339(&expires_at_str)
        .map_err(|e| crate::EngError::Internal(format!("invalid expires_at: {}", e)))?
        .with_timezone(&Utc);

    let decided_at = decided_at_str
        .map(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|e| crate::EngError::Internal(format!("invalid decided_at: {}", e)))
        })
        .transpose()?;

    Ok(Approval {
        id,
        action,
        context,
        requester,
        status,
        decision_by,
        decision_reason,
        created_at,
        expires_at,
        decided_at,
        user_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_get_approval() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let req = CreateApprovalRequest {
            action: "DELETE /memories/123".to_string(),
            context: Some(r#"{"memory_id": 123}"#.to_string()),
            requester: "test-agent".to_string(),
            window_secs: None,
        };

        let approval = create_approval(&db, &req, 1).await.expect("create");
        assert_eq!(approval.status, ApprovalStatus::Pending);
        assert_eq!(approval.action, "DELETE /memories/123");
        assert!(approval.seconds_remaining() > 0);
        assert!(approval.seconds_remaining() <= 120);

        let fetched = get_approval(&db, &approval.id, 1)
            .await
            .expect("get")
            .expect("should exist");
        assert_eq!(fetched.id, approval.id);
        assert_eq!(fetched.status, ApprovalStatus::Pending);
    }

    #[tokio::test]
    async fn test_decide_approval() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let req = CreateApprovalRequest {
            action: "DELETE /memories/456".to_string(),
            context: None,
            requester: "test-agent".to_string(),
            window_secs: Some(300),
        };

        let approval = create_approval(&db, &req, 1).await.expect("create");

        let decide_req = DecideRequest {
            decision: ApprovalDecision::Approved,
            decided_by: Some("admin".to_string()),
            reason: Some("Looks good".to_string()),
        };

        let decided = decide(&db, &approval.id, &decide_req, 1)
            .await
            .expect("decide");
        assert_eq!(decided.status, ApprovalStatus::Approved);
        assert_eq!(decided.decision_by, Some("admin".to_string()));
        assert!(decided.decided_at.is_some());
    }

    #[tokio::test]
    async fn test_list_pending() {
        let db = Database::connect_memory().await.expect("in-memory db");

        // Create two approvals
        let req1 = CreateApprovalRequest {
            action: "action1".to_string(),
            context: None,
            requester: "agent1".to_string(),
            window_secs: Some(60),
        };
        let req2 = CreateApprovalRequest {
            action: "action2".to_string(),
            context: None,
            requester: "agent2".to_string(),
            window_secs: Some(120),
        };

        create_approval(&db, &req1, 1).await.expect("create1");
        create_approval(&db, &req2, 1).await.expect("create2");

        let pending = list_pending(&db, 1).await.expect("list");
        assert_eq!(pending.len(), 2);
        // Should be ordered by expires_at (soonest first)
        assert!(pending[0].expires_at <= pending[1].expires_at);
    }

    #[tokio::test]
    async fn test_cannot_decide_twice() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let req = CreateApprovalRequest {
            action: "test action".to_string(),
            context: None,
            requester: "agent".to_string(),
            window_secs: None,
        };

        let approval = create_approval(&db, &req, 1).await.expect("create");

        let decide_req = DecideRequest {
            decision: ApprovalDecision::Denied,
            decided_by: None,
            reason: None,
        };

        decide(&db, &approval.id, &decide_req, 1)
            .await
            .expect("first decide");

        // Second decide should fail
        let result = decide(&db, &approval.id, &decide_req, 1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_expire_stale() {
        let db = Database::connect_memory().await.expect("in-memory db");

        // Create an approval with 1 second window
        let req = CreateApprovalRequest {
            action: "quick action".to_string(),
            context: None,
            requester: "agent".to_string(),
            window_secs: Some(1),
        };

        let approval = create_approval(&db, &req, 1).await.expect("create");

        // Wait for expiry
        tokio::time::sleep(tokio::time::Duration::from_millis(1100)).await;

        // Expire stale
        let expired_count = expire_stale(&db).await.expect("expire");
        assert_eq!(expired_count, 1);

        // Verify status changed
        let fetched = get_approval(&db, &approval.id, 1)
            .await
            .expect("get")
            .expect("should exist");
        assert_eq!(fetched.status, ApprovalStatus::Expired);
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        let db = Database::connect_memory().await.expect("in-memory db");

        let req = CreateApprovalRequest {
            action: "user1 action".to_string(),
            context: None,
            requester: "agent".to_string(),
            window_secs: None,
        };

        let approval = create_approval(&db, &req, 1).await.expect("create");

        // User 2 should not see user 1's approval
        let fetched = get_approval(&db, &approval.id, 2).await.expect("get");
        assert!(fetched.is_none());

        // User 2's pending list should be empty
        let pending = list_pending(&db, 2).await.expect("list");
        assert!(pending.is_empty());
    }
}
