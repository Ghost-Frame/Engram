use serde::Deserialize;

#[derive(Deserialize)]
pub struct EnrollBody {
    pub tier: String,
    pub algo: String,
    pub pubkey_pem: String,
    pub host_label: String,
    pub label: Option<String>,
    pub serial: Option<String>,
    pub sig_hex: String,
}

#[derive(Deserialize)]
pub struct RevokeBody {
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct ListParams {
    pub active_only: Option<bool>,
}
