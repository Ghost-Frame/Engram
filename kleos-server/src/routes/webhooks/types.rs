use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct CreateWebhookBody {
    pub url: String,
    pub events: Option<Vec<String>>,
    pub secret: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TestWebhookBody {
    pub event: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DeadLetterQuery {
    pub limit: Option<i64>,
}
