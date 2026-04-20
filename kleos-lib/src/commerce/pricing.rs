use rusqlite::params;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::db::Database;
use crate::{EngError, Result};

use super::types::{ServicePricing, VolumeDiscount};

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

/// Get pricing for a specific service.
pub async fn get_service_pricing(db: &Database, service_id: &str) -> Result<ServicePricing> {
    let sid = service_id.to_string();
    db.read(move |conn| {
        conn.query_row(
            "SELECT id, service_id, base_amount, currency, chain, chain_id,
                    is_active, created_at, updated_at
             FROM service_pricing WHERE service_id = ?1 AND is_active = 1",
            params![sid],
            |row| {
                Ok(ServicePricing {
                    id: row.get(0)?,
                    service_id: row.get(1)?,
                    base_amount: Decimal::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(Decimal::ZERO),
                    currency: row.get(3)?,
                    chain: row.get(4)?,
                    chain_id: row.get(5)?,
                    is_active: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                EngError::NotFound(format!("no pricing for service: {}", sid))
            }
            other => EngError::DatabaseMessage(other.to_string()),
        })
    })
    .await
}

/// List all active service pricing entries.
pub async fn list_service_pricing(db: &Database) -> Result<Vec<ServicePricing>> {
    db.read(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, service_id, base_amount, currency, chain, chain_id,
                        is_active, created_at, updated_at
                 FROM service_pricing WHERE is_active = 1
                 ORDER BY service_id",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(ServicePricing {
                    id: row.get(0)?,
                    service_id: row.get(1)?,
                    base_amount: Decimal::from_str(&row.get::<_, String>(2)?)
                        .unwrap_or(Decimal::ZERO),
                    currency: row.get(3)?,
                    chain: row.get(4)?,
                    chain_id: row.get(5)?,
                    is_active: row.get(6)?,
                    created_at: row.get(7)?,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

/// Get volume discounts for a service, ordered by min_calls ascending.
pub async fn get_volume_discounts(db: &Database, service_id: &str) -> Result<Vec<VolumeDiscount>> {
    let sid = service_id.to_string();
    db.read(move |conn| {
        let mut stmt = conn
            .prepare(
                "SELECT id, service_id, min_calls, amount
                 FROM volume_discounts WHERE service_id = ?1
                 ORDER BY min_calls ASC",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let rows = stmt
            .query_map(params![sid], |row| {
                Ok(VolumeDiscount {
                    id: row.get(0)?,
                    service_id: row.get(1)?,
                    min_calls: row.get(2)?,
                    amount: Decimal::from_str(&row.get::<_, String>(3)?)
                        .unwrap_or(Decimal::ZERO),
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))
    })
    .await
}

/// Compute the effective price for a service given a caller's usage count
/// in the current billing period. Falls back to base_amount if no discounts
/// apply.
pub async fn compute_price(
    db: &Database,
    service_id: &str,
    caller_call_count: i64,
) -> Result<(Decimal, Option<String>)> {
    let pricing = get_service_pricing(db, service_id).await?;
    let discounts = get_volume_discounts(db, service_id).await?;

    // Find the highest qualifying discount tier.
    let mut effective = pricing.base_amount;
    let mut discount_name: Option<String> = None;

    for d in discounts.iter().rev() {
        if caller_call_count >= d.min_calls {
            effective = d.amount;
            discount_name = Some(format!("{}+ calls", d.min_calls));
            break;
        }
    }

    Ok((effective, discount_name))
}

// ---------------------------------------------------------------------------
// Write (admin)
// ---------------------------------------------------------------------------

/// Upsert a service pricing entry.
pub async fn upsert_service_pricing(
    db: &Database,
    service_id: &str,
    base_amount: Decimal,
    currency: &str,
    chain: &str,
    chain_id: i64,
) -> Result<()> {
    let sid = service_id.to_string();
    let amt = base_amount.to_string();
    let cur = currency.to_string();
    let ch = chain.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO service_pricing (service_id, base_amount, currency, chain, chain_id)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(service_id) DO UPDATE SET
                base_amount = excluded.base_amount,
                currency = excluded.currency,
                chain = excluded.chain,
                chain_id = excluded.chain_id,
                updated_at = datetime('now')",
            params![sid, amt, cur, ch, chain_id],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}
