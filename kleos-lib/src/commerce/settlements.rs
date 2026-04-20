use rusqlite::params;
use rust_decimal::Decimal;
use std::str::FromStr;
use uuid::Uuid;

use crate::db::Database;
use crate::{EngError, Result};

use super::types::{AccountBalance, PaymentMethod, PaymentSettlement, SettlementStatus};

// ---------------------------------------------------------------------------
// Create settlement
// ---------------------------------------------------------------------------

/// Record a settlement for a settled quote.
pub async fn create_settlement(
    db: &Database,
    quote_id: &str,
    user_id: Option<i64>,
    wallet_address: Option<&str>,
    amount: Decimal,
    currency: &str,
    payment_method: PaymentMethod,
    tx_hash: Option<&str>,
    block_number: Option<i64>,
) -> Result<PaymentSettlement> {
    let id = format!("stl_{}", Uuid::new_v4().as_simple());
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let amt_str = amount.to_string();

    let settlement = PaymentSettlement {
        id: id.clone(),
        quote_id: quote_id.to_string(),
        user_id,
        wallet_address: wallet_address.map(|s| s.to_string()),
        amount,
        currency: currency.to_string(),
        payment_method,
        tx_hash: tx_hash.map(|s| s.to_string()),
        block_number,
        status: SettlementStatus::Confirmed,
        created_at: now.clone(),
        confirmed_at: Some(now.clone()),
    };

    let qid = quote_id.to_string();
    let cur = currency.to_string();
    let pm = payment_method.as_str().to_string();
    let txh = tx_hash.map(|s| s.to_string());
    let wa = wallet_address.map(|s| s.to_string());

    db.write(move |conn| {
        conn.execute(
            "INSERT INTO payment_settlements
                (id, quote_id, user_id, wallet_address, amount, currency,
                 payment_method, tx_hash, block_number, status, created_at, confirmed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![id, qid, user_id, wa, amt_str, cur, pm, txh, block_number, "confirmed", now, now],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await?;

    Ok(settlement)
}

// ---------------------------------------------------------------------------
// Account balance
// ---------------------------------------------------------------------------

/// Get or create an account balance for a user.
pub async fn get_balance(db: &Database, user_id: i64) -> Result<AccountBalance> {
    db.read(move |conn| {
        match conn.query_row(
            "SELECT user_id, balance, currency, updated_at
             FROM account_balances WHERE user_id = ?1",
            params![user_id],
            |row| {
                Ok(AccountBalance {
                    user_id: row.get(0)?,
                    balance: Decimal::from_str(&row.get::<_, String>(1)?)
                        .unwrap_or(Decimal::ZERO),
                    currency: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        ) {
            Ok(b) => Ok(b),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(AccountBalance {
                user_id,
                balance: Decimal::ZERO,
                currency: "USDC".to_string(),
                updated_at: String::new(),
            }),
            Err(e) => Err(EngError::DatabaseMessage(e.to_string())),
        }
    })
    .await
}

/// Deduct from a user's prepaid balance. Returns error if insufficient funds.
pub async fn deduct_balance(db: &Database, user_id: i64, amount: Decimal) -> Result<Decimal> {
    let amt_str = amount.to_string();
    db.write(move |conn| {
        // Read current balance.
        let current: String = conn
            .query_row(
                "SELECT balance FROM account_balances WHERE user_id = ?1",
                params![user_id],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    EngError::InvalidInput("no prepaid balance account".to_string())
                }
                other => EngError::DatabaseMessage(other.to_string()),
            })?;

        let current_dec =
            Decimal::from_str(&current).unwrap_or(Decimal::ZERO);
        let deduct = Decimal::from_str(&amt_str).unwrap_or(Decimal::ZERO);

        if current_dec < deduct {
            return Err(EngError::InvalidInput(format!(
                "insufficient balance: have {}, need {}",
                current_dec, deduct
            )));
        }

        let new_balance = current_dec - deduct;
        conn.execute(
            "UPDATE account_balances SET balance = ?2, updated_at = datetime('now')
             WHERE user_id = ?1",
            params![user_id, new_balance.to_string()],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;

        Ok(new_balance)
    })
    .await
}

/// Record daily spend for policy enforcement.
pub async fn record_daily_spend(
    db: &Database,
    user_id: i64,
    amount: Decimal,
) -> Result<()> {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let amt_str = amount.to_string();
    db.write(move |conn| {
        conn.execute(
            "INSERT INTO daily_spend (user_id, date, total_amount, call_count)
             VALUES (?1, ?2, ?3, 1)
             ON CONFLICT(user_id, date) DO UPDATE SET
                total_amount = CAST(
                    CAST(daily_spend.total_amount AS REAL) + CAST(?3 AS REAL)
                    AS TEXT),
                call_count = daily_spend.call_count + 1,
                updated_at = datetime('now')",
            params![user_id, date, amt_str],
        )
        .map_err(|e| EngError::DatabaseMessage(e.to_string()))?;
        Ok(())
    })
    .await
}

/// Get today's spend for a user.
pub async fn get_daily_spend(db: &Database, user_id: i64) -> Result<Decimal> {
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    db.read(move |conn| {
        match conn.query_row(
            "SELECT total_amount FROM daily_spend WHERE user_id = ?1 AND date = ?2",
            params![user_id, date],
            |row| row.get::<_, String>(0),
        ) {
            Ok(s) => Ok(Decimal::from_str(&s).unwrap_or(Decimal::ZERO)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Decimal::ZERO),
            Err(e) => Err(EngError::DatabaseMessage(e.to_string())),
        }
    })
    .await
}
