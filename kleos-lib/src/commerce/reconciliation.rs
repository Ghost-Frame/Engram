use rusqlite::params;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::db::Database;
use crate::{EngError, Result};

use super::types::{PaymentMethodCounts, ReconciliationResponse, ServiceSpend};

/// Get reconciliation report for a user on a given date (YYYY-MM-DD).
/// If date is None, uses today.
pub async fn get_reconciliation(
    db: &Database,
    user_id: i64,
    date: Option<&str>,
) -> Result<ReconciliationResponse> {
    let d = date
        .map(|s| s.to_string())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let date_prefix = format!("{}%", d);

    db.read(move |conn| {
        // Per-service breakdown from settlements.
        let mut stmt = conn
            .prepare(
                "SELECT pq.service_id, COUNT(*), COALESCE(SUM(CAST(ps.amount AS REAL)), 0)
                 FROM payment_settlements ps
                 JOIN payment_quotes pq ON ps.quote_id = pq.id
                 WHERE ps.user_id = ?1
                   AND ps.status = 'confirmed'
                   AND ps.created_at LIKE ?2
                 GROUP BY pq.service_id
                 ORDER BY pq.service_id",
            )
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let breakdown: Vec<ServiceSpend> = stmt
            .query_map(params![user_id, date_prefix], |row| {
                Ok(ServiceSpend {
                    service: row.get(0)?,
                    calls: row.get(1)?,
                    amount: Decimal::from_str(&format!("{:.6}", row.get::<_, f64>(2)?))
                        .unwrap_or(Decimal::ZERO),
                })
            })
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        let total_spent = breakdown.iter().fold(Decimal::ZERO, |acc, s| acc + s.amount);

        // Quote counts.
        let (quotes_created, quotes_expired, quotes_settled): (i64, i64, i64) = conn
            .query_row(
                "SELECT
                    COUNT(*),
                    SUM(CASE WHEN status = 'expired' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN status = 'settled' THEN 1 ELSE 0 END)
                 FROM payment_quotes
                 WHERE user_id = ?1 AND created_at LIKE ?2",
                params![user_id, date_prefix],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((0, 0, 0));

        // Payment method counts.
        let (balance_count, x402_count): (i64, i64) = conn
            .query_row(
                "SELECT
                    SUM(CASE WHEN payment_method = 'balance' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN payment_method = 'x402' THEN 1 ELSE 0 END)
                 FROM payment_settlements
                 WHERE user_id = ?1 AND status = 'confirmed' AND created_at LIKE ?2",
                params![user_id, date_prefix],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, 0));

        Ok(ReconciliationResponse {
            period: d,
            total_spent,
            currency: "USDC".to_string(),
            breakdown,
            quotes_created,
            quotes_expired,
            quotes_settled,
            payment_methods: PaymentMethodCounts {
                balance: balance_count,
                x402: x402_count,
            },
        })
    })
    .await
}
