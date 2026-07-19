use serde::Deserialize;

/// Body for `POST /webhooks`.
#[derive(Debug, Deserialize)]
pub(super) struct CreateWebhookBody {
    pub url: String,
    /// Accepted and stored, but not currently matched against any live event
    /// stream: only the test-fire route ever delivers to a registered URL.
    /// Use Axon subscriptions for wired event delivery.
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
