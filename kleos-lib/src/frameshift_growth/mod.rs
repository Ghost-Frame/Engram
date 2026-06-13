//! Frameshift cross-machine growth log (server-side, Component 4).
//!
//! A single reserved tenant (`frameshift-growth`) holds every authenticated
//! user's growth entries, row-scoped by `user_id`. Append-only and deduped on
//! `UNIQUE(user_id, content_hash)`, so `POST /frameshift-growth` is idempotent.
//! The autoincrement `id` is the monotonic since-cursor for incremental pull.
//! Mirrors the handoffs model; no decay, consolidation, or GC by design.

use crate::db::Database;
use crate::Result;
use rusqlite::types::Value as SqlValue;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Feature gate for the Frameshift growth tenant.
///
/// The growth log ships dormant: the `/frameshift-growth/*` routes are mounted
/// and the reserved tenant shard is pre-warmed only when this returns `true`.
/// Enable with `KLEOS_FRAMESHIFT_GROWTH=1`. This is a new feature with no legacy
/// `ENGRAM_` name, so it is read directly under the `KLEOS_` prefix rather than
/// through the dual-prefix `kleos_env` fallback. Default off keeps the reserved
/// tenant from being created until opted in, unlike the always-on `handoffs`
/// tenant this module otherwise mirrors.
pub fn enabled() -> bool {
    std::env::var("KLEOS_FRAMESHIFT_GROWTH").as_deref() == Ok("1")
}

/// A stored growth entry as returned to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrowthEntry {
    pub id: i64,
    pub user_id: i64,
    pub created_at: String,
    pub persona: Option<String>,
    pub project_id: Option<String>,
    pub scope: Option<String>,
    pub content: String,
    pub metadata: Option<String>,
    pub host: Option<String>,
    pub content_hash: String,
}

/// Parameters for storing a growth entry. `content_hash` is client-supplied
/// (the local log already carries it); when absent it is computed from content.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct StoreParams {
    pub persona: Option<String>,
    pub project_id: Option<String>,
    pub scope: Option<String>,
    pub content: String,
    pub metadata: Option<String>,
    pub host: Option<String>,
    pub content_hash: Option<String>,
}

/// Filters for the incremental list/pull endpoint.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GrowthFilters {
    pub persona: Option<String>,
    pub project_id: Option<String>,
    pub scope: Option<String>,
    /// Return only rows with `id` strictly greater than this cursor.
    pub since: Option<i64>,
    pub limit: Option<i64>,
}

/// Outcome of a store: the row id, and whether it was a dedup no-op.
#[derive(Debug, Clone, Serialize)]
pub struct StoreResult {
    pub id: i64,
    pub skipped: bool,
}

/// Growth-log accessor bound to the reserved `frameshift-growth` tenant shard.
pub struct FrameshiftGrowthDb {
    db: Arc<Database>,
}

/// Derive a stable 16-char content hash (matches the handoffs convention).
fn compute_content_hash(content: &str) -> String {
    let hash = Sha256::digest(content.as_bytes());
    format!("{:x}", hash)[..16].to_string()
}

impl FrameshiftGrowthDb {
    /// Bind to a resolved growth-tenant database handle.
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Idempotent store. Returns the existing row id with `skipped = true` when
    /// `(user_id, content_hash)` already exists; never errors on a duplicate.
    pub async fn store(&self, params: StoreParams, user_id: i64) -> Result<StoreResult> {
        let content_hash = params
            .content_hash
            .clone()
            .unwrap_or_else(|| compute_content_hash(&params.content));

        self.db
            .write(move |conn| {
                // ON CONFLICT DO NOTHING makes the insert atomic against the
                // UNIQUE(user_id, content_hash) index -- no read-then-write race.
                let changed = conn.execute(
                    "INSERT INTO frameshift_growth \
                     (user_id, persona, project_id, scope, content, metadata, host, content_hash) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                     ON CONFLICT(user_id, content_hash) DO NOTHING",
                    rusqlite::params![
                        user_id,
                        params.persona,
                        params.project_id,
                        params.scope,
                        params.content,
                        params.metadata,
                        params.host,
                        content_hash,
                    ],
                )?;
                if changed == 1 {
                    Ok(StoreResult {
                        id: conn.last_insert_rowid(),
                        skipped: false,
                    })
                } else {
                    let id: i64 = conn.query_row(
                        "SELECT id FROM frameshift_growth WHERE user_id = ?1 AND content_hash = ?2",
                        rusqlite::params![user_id, content_hash],
                        |r| r.get(0),
                    )?;
                    Ok(StoreResult { id, skipped: true })
                }
            })
            .await
    }

    /// Incremental list scoped to the caller, ordered by the `id` cursor so a
    /// client can pull everything after its last-seen id.
    pub async fn list(&self, filters: GrowthFilters, user_id: i64) -> Result<Vec<GrowthEntry>> {
        let limit = filters.limit.unwrap_or(100).clamp(1, 1000);

        self.db
            .read(move |conn| {
                let mut conds = vec!["user_id = ?".to_string()];
                let mut args: Vec<SqlValue> = vec![SqlValue::Integer(user_id)];
                if let Some(p) = filters.persona {
                    conds.push("persona = ?".into());
                    args.push(SqlValue::Text(p));
                }
                if let Some(p) = filters.project_id {
                    conds.push("project_id = ?".into());
                    args.push(SqlValue::Text(p));
                }
                if let Some(s) = filters.scope {
                    conds.push("scope = ?".into());
                    args.push(SqlValue::Text(s));
                }
                if let Some(since) = filters.since {
                    conds.push("id > ?".into());
                    args.push(SqlValue::Integer(since));
                }
                args.push(SqlValue::Integer(limit));
                let sql = format!(
                    "SELECT id, user_id, created_at, persona, project_id, scope, content, \
                     metadata, host, content_hash FROM frameshift_growth WHERE {} \
                     ORDER BY id ASC LIMIT ?",
                    conds.join(" AND ")
                );
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map(rusqlite::params_from_iter(args), row_to_entry)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await
    }

    /// Substring content search scoped to the caller (LIKE; FTS is YAGNI here).
    pub async fn search(&self, query: &str, user_id: i64, limit: i64) -> Result<Vec<GrowthEntry>> {
        let limit = limit.clamp(1, 1000);
        let pat = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        self.db
            .read(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, user_id, created_at, persona, project_id, scope, content, \
                     metadata, host, content_hash FROM frameshift_growth \
                     WHERE user_id = ?1 AND content LIKE ?2 ESCAPE '\\' \
                     ORDER BY created_at DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(rusqlite::params![user_id, pat, limit], row_to_entry)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await
    }

    /// Highest row id for the caller, the cursor a client advances past. Zero
    /// when the caller has no entries yet.
    pub async fn max_cursor(&self, user_id: i64) -> Result<i64> {
        self.db
            .read(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT MAX(id) FROM frameshift_growth WHERE user_id = ?1",
                        rusqlite::params![user_id],
                        |r| r.get::<_, Option<i64>>(0),
                    )
                    .optional()?
                    .flatten()
                    .unwrap_or(0))
            })
            .await
    }
}

/// Map a SELECT row (column order fixed above) into a [`GrowthEntry`].
fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<GrowthEntry> {
    Ok(GrowthEntry {
        id: row.get(0)?,
        user_id: row.get(1)?,
        created_at: row.get(2)?,
        persona: row.get(3)?,
        project_id: row.get(4)?,
        scope: row.get(5)?,
        content: row.get(6)?,
        metadata: row.get(7)?,
        host: row.get(8)?,
        content_hash: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn growth_db() -> FrameshiftGrowthDb {
        let db = Database::connect_memory().await.expect("memory db");
        FrameshiftGrowthDb::new(std::sync::Arc::new(db))
    }

    fn params(content: &str) -> StoreParams {
        StoreParams {
            content: content.to_string(),
            persona: Some("security".into()),
            project_id: Some("kleos".into()),
            scope: Some("workspace".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn store_is_idempotent_per_user_and_hash() {
        let g = growth_db().await;
        let first = g.store(params("learned X"), 1).await.expect("store");
        assert!(!first.skipped, "first insert is not a dedup");
        let again = g.store(params("learned X"), 1).await.expect("store again");
        assert!(again.skipped, "same content+user must dedup");
        assert_eq!(first.id, again.id, "dedup returns the existing id");
        // Same content under a different user is a distinct row.
        let other = g.store(params("learned X"), 2).await.expect("store u2");
        assert!(!other.skipped, "different user is not a dedup");
    }

    #[tokio::test]
    async fn list_is_user_scoped_and_cursor_paginates() {
        let g = growth_db().await;
        let a = g.store(params("a"), 1).await.unwrap().id;
        let _b = g.store(params("b"), 1).await.unwrap().id;
        g.store(params("other-user"), 2).await.unwrap();

        // User 1 sees only their two entries, not user 2's.
        let u1 = g.list(GrowthFilters::default(), 1).await.expect("list u1");
        assert_eq!(u1.len(), 2, "user 1 must not see user 2's growth");
        assert!(u1.iter().all(|e| e.user_id == 1));

        // since-cursor returns only entries after the first row's id.
        let after_a = g
            .list(
                GrowthFilters {
                    since: Some(a),
                    ..Default::default()
                },
                1,
            )
            .await
            .expect("list since");
        assert_eq!(after_a.len(), 1, "only the entry after the cursor");
        assert!(after_a[0].id > a);

        // User 2's listing is isolated.
        let u2 = g.list(GrowthFilters::default(), 2).await.expect("list u2");
        assert_eq!(u2.len(), 1);
    }

    #[tokio::test]
    async fn search_is_user_scoped() {
        let g = growth_db().await;
        g.store(params("alpha secret note"), 1).await.unwrap();
        g.store(params("beta note"), 2).await.unwrap();
        let hits = g.search("secret", 1, 50).await.expect("search");
        assert_eq!(hits.len(), 1);
        // User 2 cannot find user 1's content.
        let none = g.search("secret", 2, 50).await.expect("search u2");
        assert!(none.is_empty());
    }
}
