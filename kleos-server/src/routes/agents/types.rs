use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct RegisterBody {
    pub name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub code_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RevokeBody {
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LinkKeyBody {
    pub key_id: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct ExecutionsQuery {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct VerifyBody {
    pub passport: Option<Value>,
    pub execution: Option<Value>,
    pub message: Option<Value>,
    pub tool_manifest: Option<Value>,
}
