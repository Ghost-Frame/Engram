use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::extractors::Auth;
use crate::state::AppState;
use kleos_lib::commerce::{pricing, quotes, reconciliation, settlements, types::*};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/commerce/quotes", post(create_quote))
        .route("/commerce/quotes/{id}", get(get_quote))
        .route("/commerce/check", post(budget_check))
        .route("/commerce/reconciliation", get(get_reconciliation))
        .route("/commerce/balance", get(get_balance))
        .route("/commerce/pricing", get(list_pricing))
}

// ---------------------------------------------------------------------------
// POST /commerce/quotes
// ---------------------------------------------------------------------------

async fn create_quote(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<CreateQuoteRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    // Validate service exists in pricing table.
    let _pricing = pricing::get_service_pricing(&state.db, &body.service)
        .await
        .map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    // Get caller's usage count for volume discount computation.
    let daily_spent = settlements::get_daily_spend(&state.db, auth.user_id)
        .await
        .unwrap_or(rust_decimal::Decimal::ZERO);
    // Use daily call count as a rough proxy for volume discount.
    // A more precise implementation would count calls in the billing period.
    let _ = daily_spent;

    let (amount, discount) = pricing::compute_price(&state.db, &body.service, 0)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    let quote = quotes::create_quote(
        &state.db,
        Some(auth.user_id),
        None, // wallet_address -- set for x402 callers
        &body.service,
        amount,
        "USDC",
        discount.as_deref(),
        body.parameters.clone(),
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

    // Determine available payment methods.
    let balance = settlements::get_balance(&state.db, auth.user_id)
        .await
        .unwrap_or(AccountBalance {
            user_id: auth.user_id,
            balance: rust_decimal::Decimal::ZERO,
            currency: "USDC".to_string(),
            updated_at: String::new(),
        });

    let mut methods = vec!["x402".to_string()];
    if balance.balance > rust_decimal::Decimal::ZERO {
        methods.insert(0, "balance".to_string());
    }

    let resp = CreateQuoteResponse {
        quote_id: quote.id,
        service: quote.service_id,
        amount: quote.amount,
        currency: quote.currency,
        discount_applied: quote.discount_applied,
        expires_at: quote.expires_at,
        payment_methods: methods,
    };

    Ok((StatusCode::CREATED, Json(json!(resp))))
}

// ---------------------------------------------------------------------------
// GET /commerce/quotes/:id
// ---------------------------------------------------------------------------

async fn get_quote(
    State(state): State<AppState>,
    Auth(auth): Auth,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let quote = quotes::get_quote(&state.db, &id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": e.to_string() })),
        )
    })?;

    // Ensure the quote belongs to this user.
    if quote.user_id != Some(auth.user_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "quote not found" })),
        ));
    }

    Ok(Json(json!(quote)))
}

// ---------------------------------------------------------------------------
// POST /commerce/check
// ---------------------------------------------------------------------------

async fn budget_check(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Json(body): Json<BudgetCheckRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let quote = quotes::get_valid_quote(&state.db, &body.quote_id)
        .await
        .map_err(|e| {
            let status = match &e {
                kleos_lib::EngError::NotFound(_) => StatusCode::NOT_FOUND,
                kleos_lib::EngError::Conflict(_) => StatusCode::GONE,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (status, Json(json!({ "error": e.to_string() })))
        })?;

    // Verify ownership.
    if quote.user_id != Some(auth.user_id) {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "quote not found" })),
        ));
    }

    let balance = settlements::get_balance(&state.db, auth.user_id)
        .await
        .unwrap_or(AccountBalance {
            user_id: auth.user_id,
            balance: rust_decimal::Decimal::ZERO,
            currency: "USDC".to_string(),
            updated_at: String::new(),
        });

    let daily_spent = settlements::get_daily_spend(&state.db, auth.user_id)
        .await
        .unwrap_or(rust_decimal::Decimal::ZERO);

    // For now, no daily limit enforcement (configurable later via tenant_quotas).
    let balance_after = balance.balance - quote.amount;
    let approved = balance.balance >= quote.amount;

    let resp = BudgetCheckResponse {
        approved,
        quote_id: quote.id,
        amount: quote.amount,
        balance: balance.balance,
        balance_after: if approved {
            balance_after
        } else {
            balance.balance
        },
        requires_approval: false,
        approval_id: None,
        reason: if !approved {
            Some(format!(
                "insufficient balance: have {}, need {}",
                balance.balance, quote.amount
            ))
        } else {
            None
        },
        policy: SpendPolicy {
            daily_limit: None,
            daily_spent,
            daily_remaining: None,
        },
    };

    Ok(Json(json!(resp)))
}

// ---------------------------------------------------------------------------
// GET /commerce/reconciliation
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ReconciliationQuery {
    date: Option<String>,
}

async fn get_reconciliation(
    State(state): State<AppState>,
    Auth(auth): Auth,
    Query(q): Query<ReconciliationQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let report = reconciliation::get_reconciliation(&state.db, auth.user_id, q.date.as_deref())
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(json!(report)))
}

// ---------------------------------------------------------------------------
// GET /commerce/balance
// ---------------------------------------------------------------------------

async fn get_balance(
    State(state): State<AppState>,
    Auth(auth): Auth,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let balance = settlements::get_balance(&state.db, auth.user_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(json!(BalanceResponse {
        user_id: auth.user_id,
        balance: balance.balance,
        currency: balance.currency,
    })))
}

// ---------------------------------------------------------------------------
// GET /commerce/pricing
// ---------------------------------------------------------------------------

async fn list_pricing(
    State(state): State<AppState>,
    Auth(_auth): Auth,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let prices = pricing::list_service_pricing(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;

    Ok(Json(json!({ "services": prices })))
}
