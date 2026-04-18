use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct PromptQuery {
    pub format: Option<String>,
    pub tokens: Option<usize>,
    pub context: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct HeaderBody {
    pub actor_model: Option<String>,
    pub actor_role: Option<String>,
    pub context: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub(super) struct GeneratePromptRequest {
    pub agent: String,
    pub task: String,
    #[serde(default)]
    pub max_tokens: Option<usize>,
    #[serde(default)]
    pub include_personality: Option<bool>,
    #[serde(default)]
    pub include_memories: Option<bool>,
    #[serde(default)]
    pub memory_limit: Option<usize>,
    #[serde(default)]
    pub include_brain: Option<bool>,
    #[serde(default)]
    pub include_growth: Option<bool>,
    #[serde(default)]
    pub include_instincts: Option<bool>,
    #[serde(default)]
    pub brain_limit: Option<usize>,
    #[serde(default)]
    pub growth_limit: Option<usize>,
}
