//! Access policy model -- defines which secrets require approval.

use kleos_cred::{CredError, Result};
use kleos_lib::db::Database;
use rusqlite::params;

/// An access policy controlling approval requirements for secrets.
#[derive(Debug, Clone)]
pub struct AccessPolicy {
    /// Row ID.
    pub id: i64,
    /// Owner user ID.
    pub user_id: i64,
    /// Namespace this policy applies to.
    pub namespace: String,
    /// Category filter (None = all categories in namespace).
    pub category: Option<String>,
    /// Secret name filter (None = all secrets in category).
    pub secret_name: Option<String>,
    /// Whether approval is required for matching secrets.
    pub require_approval: bool,
    /// Which resolve modes are allowed (text, proxy, raw, exec, verify,
    /// sign, derive).
    pub allowed_modes: Vec<String>,
    /// Absolute argv[0] paths exec mode may spawn. None = exec never
    /// allowed by this policy, even when allowed_modes names "exec".
    pub exec_allowlist: Option<Vec<String>>,
    /// When the policy was created.
    pub created_at: String,
}

/// Serializable policy for JSON responses.
impl AccessPolicy {
    /// Convert to a serde_json::Value for API responses.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "namespace": self.namespace,
            "category": self.category,
            "secret_name": self.secret_name,
            "require_approval": self.require_approval,
            "allowed_modes": self.allowed_modes,
            "exec_allowlist": self.exec_allowlist,
            "created_at": self.created_at,
        })
    }
}

/// Find the most specific matching policy for a secret access.
///
/// Specificity order: namespace+category+secret > namespace+category > namespace only.
pub async fn find_matching_policy(
    db: &Database,
    user_id: i64,
    namespace: &str,
    category: &str,
    secret_name: &str,
) -> Result<Option<AccessPolicy>> {
    let ns = namespace.to_string();
    let cat = category.to_string();
    let sec = secret_name.to_string();

    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, user_id, namespace, category, secret_name,
                    require_approval, allowed_modes, created_at, exec_allowlist
             FROM phylax_access_policies
             WHERE user_id = ?1 AND namespace = ?2
               AND (category IS NULL OR category = ?3)
               AND (secret_name IS NULL OR secret_name = ?4)
             ORDER BY
               (CASE WHEN secret_name IS NOT NULL THEN 0 ELSE 1 END),
               (CASE WHEN category IS NOT NULL THEN 0 ELSE 1 END)
             LIMIT 1",
        )?;
        let policy = stmt
            .query_row(params![user_id, ns, cat, sec], row_to_policy)
            .ok();
        Ok(policy)
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

/// Create a new access policy.
// Every column of the policy row is an explicit parameter; a builder would
// add ceremony without removing the coupling to the schema.
#[allow(clippy::too_many_arguments)]
pub async fn create_policy(
    db: &Database,
    user_id: i64,
    namespace: &str,
    category: Option<&str>,
    secret_name: Option<&str>,
    require_approval: bool,
    allowed_modes: &[String],
    exec_allowlist: Option<&[String]>,
) -> Result<AccessPolicy> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let ns = namespace.to_string();
    let cat = category.map(|s| s.to_string());
    let sec = secret_name.map(|s| s.to_string());
    let modes_json = serde_json::to_string(allowed_modes).unwrap_or_default();
    let exec_json = exec_allowlist.map(|a| serde_json::to_string(a).unwrap_or_default());
    let exec_json2 = exec_json.clone();
    let now2 = now.clone();

    let id = db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO phylax_access_policies
                 (user_id, namespace, category, secret_name,
                  require_approval, allowed_modes, created_at, exec_allowlist)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    user_id,
                    ns,
                    cat,
                    sec,
                    require_approval as i32,
                    modes_json,
                    now2,
                    exec_json2
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    Ok(AccessPolicy {
        id,
        user_id,
        namespace: namespace.to_string(),
        category: category.map(|s| s.to_string()),
        secret_name: secret_name.map(|s| s.to_string()),
        require_approval,
        allowed_modes: allowed_modes.to_vec(),
        exec_allowlist: exec_allowlist.map(|a| a.to_vec()),
        created_at: now,
    })
}

/// List all policies for a user.
pub async fn list_policies(db: &Database, user_id: i64) -> Result<Vec<AccessPolicy>> {
    db.read(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, user_id, namespace, category, secret_name,
                    require_approval, allowed_modes, created_at, exec_allowlist
             FROM phylax_access_policies
             WHERE user_id = ?1
             ORDER BY namespace, category, secret_name",
        )?;
        let rows = stmt.query_map(params![user_id], row_to_policy)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    })
    .await
    .map_err(|e| CredError::Database(e.to_string()))
}

/// Update a policy's approval requirement and allowed modes.
pub async fn update_policy(
    db: &Database,
    id: i64,
    require_approval: bool,
    allowed_modes: &[String],
    exec_allowlist: Option<&[String]>,
) -> Result<()> {
    let modes_json = serde_json::to_string(allowed_modes).unwrap_or_default();
    let exec_json = exec_allowlist.map(|a| serde_json::to_string(a).unwrap_or_default());
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "UPDATE phylax_access_policies
                 SET require_approval = ?1, allowed_modes = ?2, exec_allowlist = ?3
                 WHERE id = ?4",
                params![require_approval as i32, modes_json, exec_json, id],
            )?)
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    if affected == 0 {
        return Err(CredError::NotFound("policy not found".into()));
    }
    Ok(())
}

/// Delete a policy by ID.
pub async fn delete_policy(db: &Database, id: i64) -> Result<()> {
    let affected = db
        .write(move |conn| {
            Ok(conn.execute(
                "DELETE FROM phylax_access_policies WHERE id = ?1",
                params![id],
            )?)
        })
        .await
        .map_err(|e| CredError::Database(e.to_string()))?;

    if affected == 0 {
        return Err(CredError::NotFound("policy not found".into()));
    }
    Ok(())
}

/// Parse a database row into an AccessPolicy struct.
fn row_to_policy(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccessPolicy> {
    let modes_json: String = row.get(6)?;
    let allowed_modes: Vec<String> =
        serde_json::from_str(&modes_json).unwrap_or_else(|_| vec!["text".into()]);
    // An unparseable allowlist degrades to None (exec denied), never to a
    // broader permission.
    let exec_allowlist: Option<Vec<String>> = row
        .get::<_, Option<String>>(8)?
        .and_then(|j| serde_json::from_str(&j).ok());
    Ok(AccessPolicy {
        id: row.get(0)?,
        user_id: row.get(1)?,
        namespace: row.get(2)?,
        category: row.get(3)?,
        secret_name: row.get(4)?,
        require_approval: row.get::<_, i32>(5)? != 0,
        allowed_modes,
        exec_allowlist,
        created_at: row.get(7)?,
    })
}
