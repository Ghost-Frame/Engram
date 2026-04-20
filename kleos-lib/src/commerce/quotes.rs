use chrono::Utc;
use rusqlite::params;
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

use crate::db::Database;
use crate::{EngError, Result};

use super::types::{PaymentQuote, QuoteStatus};

/// Default quote time-to-live: 5 minutes.
const QUOTE_TTL_SECS: i64 = 300;

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

/// Create a new payment quote, locking the price for QUOTE_TTL_SECS.
pub async fn create_quote(
    db: &Database,
    user_id: Option<i64>,
    wallet_address: Option<&str>,
    service_id: &str,
    amount: Decimal,
    currency: &str,
    discount_applied: Option<&str>,
    parameters: Option<serde_json::Value>,
) -> Result<PaymentQuote> {
    let id = format!("q_{}", Uuid::new_v4().as_simple());
    let now = Utc::now();
    let created_at = now.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let expires_at = (now + chrono::Duration::seconds(QUOTE_TTL_SECS))
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let amt_str = amount.to_string();
    let params_json = parameters
        .as_ref()
        .map(|v| serde_json::to_string(v).unwrap_or_default());

    let quote = PaymentQuote {
        id: id.clone(),
        user_id,
        wallet_address: wallet_address.map(|s| s.to_string()),
        service_id: service_id.to_string(),
        amount,
        currency: currency.to_string(),
        discount_applied: discount_applied.map(|s| s.to_string()),
        status: QuoteStatus::Pending,
        parameters: parameters.clone(),
        created_at: created_at.clone(),
        expires_at: expires_at.clone(),
        settled_at: None,
    };

    let sid = service_id.to_string();
    let cur = currency.to_string();
    let disc = discount_applied.map(|s| s.to_string());
    let wa = wallet_address.map(|s| s.to_string());

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO payment_quotes
                (id, user_id, wallet_address, service_id, amount, currency,
                 discount_applied, status, parameters, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id,
                user_id,
                wa,
                sid,
                amt_str,
                cur,
                disc,
                "pending",
                params_json,
                created_at,
                expires_at,
            ],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    Ok(quote)
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Get a quote by ID. Returns NotFound if the quote does not exist.
pub async fn get_quote(db: &Database, quote_id: &str) -> Result<PaymentQuote> {
    let qid = quote_id.to_string();
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, user_id, wallet_address, service_id, amount, currency,
                    discount_applied, status, parameters, created_at, expires_at, settled_at
             FROM payment_quotes WHERE id = ?1",
            params![qid],
            |row| {
                let params_str: Option<String> = row.get(8)?;
                Ok(PaymentQuote {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    wallet_address: row.get(2)?,
                    service_id: row.get(3)?,
                    amount: Decimal::from_str(&row.get::<_, String>(4)?)
                        .unwrap_or(Decimal::ZERO),
                    currency: row.get(5)?,
                    discount_applied: row.get(6)?,
                    status: QuoteStatus::from_str(&row.get::<_, String>(7)?)
                        .unwrap_or(QuoteStatus::Pending),
                    parameters: params_str
                        .and_then(|s| serde_json::from_str(&s).ok()),
                    created_at: row.get(9)?,
                    expires_at: row.get(10)?,
                    settled_at: row.get(11)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                EngError::NotFound(format!("quote not found: {}", qid))
            }
            other => EngError::DatabaseMessage(other.to_string()),
        })
    })
    .await
}

/// Get a quote and validate it's usable (pending + not expired).
pub async fn get_valid_quote(db: &Database, quote_id: &str) -> Result<PaymentQuote> {
    let quote = get_quote(db, quote_id).await?;

    if quote.status != QuoteStatus::Pending {
        return Err(EngError::Conflict(format!(
            "quote {} is {}, not pending",
            quote_id,
            quote.status.as_str()
        )));
    }

    // Check expiry.
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    if quote.expires_at < now {
        // Best-effort mark as expired -- don't block on failure.
        let _ = mark_expired(db, quote_id).await;
        return Err(EngError::Conflict(format!("quote {} has expired", quote_id)));
    }

    Ok(quote)
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// Mark a quote as settled (CAS: only if currently pending).
pub async fn settle_quote(db: &Database, quote_id: &str) -> Result<()> {
    let qid = quote_id.to_string();
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    db.write(move |conn| {
        let rows = conn
            .execute(
                "UPDATE payment_quotes SET status = 'settled', settled_at = ?2
                 WHERE id = ?1 AND status = 'pending'",
                params![qid, now],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        if rows == 0 {
            return Err(EngError::Conflict(format!(
                "quote {} is not pending (may be expired or already settled)",
                qid
            )));
        }
        Ok(())
    })
    .await
}

/// Mark a quote as expired.
pub async fn mark_expired(db: &Database, quote_id: &str) -> Result<()> {
    let qid = quote_id.to_string();
    db.write(move |conn| {
        conn.execute(
            "UPDATE payment_quotes SET status = 'expired'
             WHERE id = ?1 AND status = 'pending'",
            params![qid],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

/// Expire all quotes that have passed their expires_at.
pub async fn expire_stale_quotes(db: &Database) -> Result<i64> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    db.write(move |conn| {
        let rows = conn
            .execute(
                "UPDATE payment_quotes SET status = 'expired'
                 WHERE status = 'pending' AND expires_at < ?1",
                params![now],
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(rows as i64)
    })
    .await
}
