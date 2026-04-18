//! Request and response DTOs for credd HTTP handlers.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use engram_cred::types::SecretData;

#[derive(Deserialize)]
pub struct ListQuery {
    pub category: Option<String>,
}

#[derive(Serialize)]
pub struct SecretListItem {
    pub service: String,
    pub key: String,
    pub secret_type: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Deserialize)]
pub struct StoreRequest {
    pub data: SecretData,
}

#[derive(Serialize)]
pub struct AgentKeyInfo {
    pub name: String,
    pub categories: Vec<String>,
    pub allow_raw: bool,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub is_valid: bool,
}

#[derive(Deserialize)]
pub struct CreateKeyRequest {
    pub name: String,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub allow_raw: bool,
}

#[derive(Deserialize)]
pub struct ResolveTextRequest {
    pub text: String,
}

#[derive(Serialize)]
pub struct ResolveTextResponse {
    pub text: String,
    pub substitutions: usize,
}

#[derive(Deserialize)]
pub struct ProxyRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<String>,
    pub secret_category: String,
    pub secret_name: String,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_scheme: Option<String>,
}

#[derive(Serialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[derive(Deserialize)]
pub struct RawRequest {
    pub category: String,
    pub name: String,
}
