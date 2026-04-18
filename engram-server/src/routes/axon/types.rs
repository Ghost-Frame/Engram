use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct PublishBody {
    pub channel: String,
    /// The plan spec says event_type but the lib uses `action`
    pub action: Option<String>,
    pub event_type: Option<String>,
    pub payload: Option<serde_json::Value>,
    pub source: Option<String>,
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct QueryEventsParams {
    pub channel: Option<String>,
    pub event_type: Option<String>,
    pub action: Option<String>,
    pub source: Option<String>,
    pub since_id: Option<i64>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateChannelBody {
    pub name: String,
    pub description: Option<String>,
    #[allow(dead_code)]
    pub retain_hours: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SubscribeBody {
    pub agent: String,
    pub channel: String,
    pub filter_type: Option<String>,
    pub webhook_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UnsubscribeBody {
    pub agent: String,
    pub channel: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ListSubscriptionsParams {
    pub agent: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PollBody {
    pub agent: String,
    pub channel: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GetCursorParams {
    pub agent: String,
    pub channel: String,
}
