use kleos_lib::conversations::AddMessageRequest;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct ListConversationsParams {
    pub limit: Option<usize>,
    pub agent: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct GetConversationParams {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum MessageBody {
    Single(AddMessageRequest),
    Batch(Vec<AddMessageRequest>),
}

#[derive(Debug, Deserialize)]
pub(super) struct ListMessagesParams {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}
