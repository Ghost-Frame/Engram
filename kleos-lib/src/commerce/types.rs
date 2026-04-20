use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Service pricing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServicePricing {
    pub id: i64,
    pub service_id: String,
    pub base_amount: Decimal,
    pub currency: String,
    pub chain: String,
    pub chain_id: i64,
    pub is_active: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeDiscount {
    pub id: i64,
    pub service_id: String,
    pub min_calls: i64,
    pub amount: Decimal,
}

// ---------------------------------------------------------------------------
// Payment quotes
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentQuote {
    pub id: String,
    pub user_id: Option<i64>,
    pub wallet_address: Option<String>,
    pub service_id: String,
    pub amount: Decimal,
    pub currency: String,
    pub discount_applied: Option<String>,
    pub status: QuoteStatus,
    pub parameters: Option<serde_json::Value>,
    pub created_at: String,
    pub expires_at: String,
    pub settled_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteStatus {
    Pending,
    Settled,
    Expired,
    Cancelled,
}

impl QuoteStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Settled => "settled",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "settled" => Some(Self::Settled),
            "expired" => Some(Self::Expired),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Settlements
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentSettlement {
    pub id: String,
    pub quote_id: String,
    pub user_id: Option<i64>,
    pub wallet_address: Option<String>,
    pub amount: Decimal,
    pub currency: String,
    pub payment_method: PaymentMethod,
    pub tx_hash: Option<String>,
    pub block_number: Option<i64>,
    pub status: SettlementStatus,
    pub created_at: String,
    pub confirmed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    Balance,
    X402,
}

impl PaymentMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Balance => "balance",
            Self::X402 => "x402",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementStatus {
    Pending,
    Confirmed,
    Failed,
}

impl SettlementStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
        }
    }
}

// ---------------------------------------------------------------------------
// Account balances
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountBalance {
    pub user_id: i64,
    pub balance: Decimal,
    pub currency: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Daily spend
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailySpend {
    pub user_id: i64,
    pub date: String,
    pub total_amount: Decimal,
    pub call_count: i64,
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateQuoteRequest {
    pub service: String,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct CreateQuoteResponse {
    pub quote_id: String,
    pub service: String,
    pub amount: Decimal,
    pub currency: String,
    pub discount_applied: Option<String>,
    pub expires_at: String,
    pub payment_methods: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct BudgetCheckRequest {
    pub quote_id: String,
}

#[derive(Debug, Serialize)]
pub struct BudgetCheckResponse {
    pub approved: bool,
    pub quote_id: String,
    pub amount: Decimal,
    pub balance: Decimal,
    pub balance_after: Decimal,
    pub requires_approval: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub policy: SpendPolicy,
}

#[derive(Debug, Serialize)]
pub struct SpendPolicy {
    pub daily_limit: Option<Decimal>,
    pub daily_spent: Decimal,
    pub daily_remaining: Option<Decimal>,
}

#[derive(Debug, Serialize)]
pub struct ReconciliationResponse {
    pub period: String,
    pub total_spent: Decimal,
    pub currency: String,
    pub breakdown: Vec<ServiceSpend>,
    pub quotes_created: i64,
    pub quotes_expired: i64,
    pub quotes_settled: i64,
    pub payment_methods: PaymentMethodCounts,
}

#[derive(Debug, Serialize)]
pub struct ServiceSpend {
    pub service: String,
    pub calls: i64,
    pub amount: Decimal,
}

#[derive(Debug, Serialize)]
pub struct PaymentMethodCounts {
    pub balance: i64,
    pub x402: i64,
}

#[derive(Debug, Serialize)]
pub struct BalanceResponse {
    pub user_id: i64,
    pub balance: Decimal,
    pub currency: String,
}
