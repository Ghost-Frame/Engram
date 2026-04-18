use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretAccessMode {
    Resolved,
    Raw,
    Scrub,
}

impl SecretAccessMode {
    pub(crate) fn tier(self) -> &'static str {
        match self {
            Self::Resolved => "resolved",
            Self::Raw => "raw",
            Self::Scrub => "scrub",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretPatternKind {
    Secret,
    Raw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretPattern {
    pub kind: SecretPatternKind,
    pub service: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyRequest {
    pub url: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub auth_header: Option<String>,
    #[serde(default)]
    pub auth_scheme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}
