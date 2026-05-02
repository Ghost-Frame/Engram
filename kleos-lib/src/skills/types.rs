use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SkillCategory {
    ToolGuide,
    #[default]
    Workflow,
    Reference,
}

impl std::fmt::Display for SkillCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToolGuide => write!(f, "tool_guide"),
            Self::Workflow => write!(f, "workflow"),
            Self::Reference => write!(f, "reference"),
        }
    }
}

impl std::str::FromStr for SkillCategory {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "tool_guide" => Ok(Self::ToolGuide),
            "workflow" => Ok(Self::Workflow),
            "reference" => Ok(Self::Reference),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown skillcategory: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SkillVisibility {
    #[default]
    Private,
    Public,
}

impl std::fmt::Display for SkillVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Private => write!(f, "private"),
            Self::Public => write!(f, "public"),
        }
    }
}

impl std::str::FromStr for SkillVisibility {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "private" => Ok(Self::Private),
            "public" => Ok(Self::Public),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown skillvisibility: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum SkillOrigin {
    #[default]
    Imported,
    Captured,
    Derived,
    Fixed,
}

impl std::fmt::Display for SkillOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Imported => write!(f, "imported"),
            Self::Captured => write!(f, "captured"),
            Self::Derived => write!(f, "derived"),
            Self::Fixed => write!(f, "fixed"),
        }
    }
}

impl std::str::FromStr for SkillOrigin {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "imported" => Ok(Self::Imported),
            "captured" => Ok(Self::Captured),
            "derived" => Ok(Self::Derived),
            "fixed" => Ok(Self::Fixed),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown skillorigin: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EvolutionType {
    Fix,
    Derived,
    Captured,
}

impl std::fmt::Display for EvolutionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fix => write!(f, "fix"),
            Self::Derived => write!(f, "derived"),
            Self::Captured => write!(f, "captured"),
        }
    }
}

impl std::str::FromStr for EvolutionType {
    type Err = crate::EngError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "fix" => Ok(Self::Fix),
            "derived" => Ok(Self::Derived),
            "captured" => Ok(Self::Captured),
            _ => Err(crate::EngError::InvalidInput(format!(
                "unknown evolutiontype: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionTrigger {
    Analysis,
    ToolDegradation,
    MetricMonitor,
}

impl std::fmt::Display for EvolutionTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Analysis => write!(f, "analysis"),
            Self::ToolDegradation => write!(f, "tool_degradation"),
            Self::MetricMonitor => write!(f, "metric_monitor"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PatchType {
    Full,
    Diff,
    Patch,
}

impl std::fmt::Display for PatchType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Diff => write!(f, "diff"),
            Self::Patch => write!(f, "patch"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillMeta {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    pub skill_id: i64,
    pub name: String,
    pub description: String,
    pub agent: String,
    pub category: String,
    pub origin: String,
    pub score: f64,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillQualityMetrics {
    pub skill_id: i64,
    pub total_executions: i32,
    pub success_count: i32,
    pub failure_count: i32,
    pub success_rate: f64,
    pub avg_duration_ms: Option<f64>,
    pub trust_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillJudgmentInput {
    pub skill_id: i64,
    pub skill_applied: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionSuggestion {
    pub evolution_type: String,
    pub target_skill_ids: Vec<i64>,
    pub category: Option<String>,
    pub direction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillEditResult {
    pub success: bool,
    pub skill_dir: String,
    pub content: String,
    pub snapshot: std::collections::HashMap<String, String>,
    pub diff: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudSkillCandidate {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub category: String,
    pub origin: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadMeta {
    pub origin: String,
    pub parent_skill_ids: Vec<i64>,
    pub tags: Vec<String>,
    pub created_by: String,
    pub change_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDependencyRecord {
    pub skill_id: i64,
    pub tool_name: String,
    pub is_optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStage {
    pub id: String,
    pub name: String,
    pub description: String,
    pub order: i32,
}

pub fn pipeline_stages() -> Vec<PipelineStage> {
    vec![
        PipelineStage {
            id: "initialize".into(),
            name: "Initialize".into(),
            description: "Load grounding client and skill registry".into(),
            order: 0,
        },
        PipelineStage {
            id: "select-skills".into(),
            name: "Skill Selection".into(),
            description: "Hybrid search for matching skills, rank".into(),
            order: 1,
        },
        PipelineStage {
            id: "skill-phase".into(),
            name: "Skill Phase".into(),
            description: "Execute task with skill context via LLM".into(),
            order: 2,
        },
        PipelineStage {
            id: "tool-fallback".into(),
            name: "Tool Fallback".into(),
            description: "Retry with tools only if skill phase fails".into(),
            order: 3,
        },
        PipelineStage {
            id: "analysis".into(),
            name: "Analysis".into(),
            description: "Run execution analyzer, persist results".into(),
            order: 4,
        },
        PipelineStage {
            id: "evolution".into(),
            name: "Evolution".into(),
            description: "Trigger FIX/DERIVED/CAPTURED based on analysis".into(),
            order: 5,
        },
    ]
}

// -- Core DTOs (moved from mod.rs; re-exported by skills/mod.rs) --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: i64,
    pub name: String,
    pub agent: String,
    pub description: Option<String>,
    pub code: String,
    pub language: String,
    pub version: i32,
    pub parent_skill_id: Option<i64>,
    pub root_skill_id: Option<i64>,
    pub trust_score: f64,
    pub success_count: i32,
    pub failure_count: i32,
    pub execution_count: i32,
    pub avg_duration_ms: Option<f64>,
    pub is_active: bool,
    pub is_deprecated: bool,
    pub metadata: Option<String>,
    pub user_id: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSkillRequest {
    pub name: String,
    pub agent: String,
    pub description: Option<String>,
    pub code: String,
    pub language: Option<String>,
    pub parent_skill_id: Option<i64>,
    pub metadata: Option<String>,
    pub user_id: Option<i64>,
    pub tags: Option<Vec<String>>,
    pub tool_deps: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSkillRequest {
    pub code: Option<String>,
    pub description: Option<String>,
    pub is_active: Option<bool>,
    pub is_deprecated: Option<bool>,
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRecord {
    pub id: i64,
    pub skill_id: i64,
    pub success: bool,
    pub duration_ms: Option<f64>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub input_hash: Option<String>,
    pub output_hash: Option<String>,
    pub metadata: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillJudgment {
    pub id: i64,
    pub skill_id: i64,
    pub judge_agent: String,
    pub score: f64,
    pub rationale: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolQuality {
    pub id: i64,
    pub tool_name: String,
    pub agent: String,
    pub success: bool,
    pub latency_ms: Option<f64>,
    pub error_type: Option<String>,
    pub created_at: String,
}

// -- Submodule DTOs --

#[derive(Debug, Clone, PartialEq)]
pub enum DetectedPatchType {
    Full,
    SearchReplace,
    MultiFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudSearchResult {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub origin: String,
    pub tags: Vec<String>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillOverview {
    pub total_skills: i64,
    pub active_skills: i64,
    pub deprecated_skills: i64,
    pub total_executions: i64,
    pub avg_trust_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillStats {
    pub id: i64,
    pub name: String,
    pub execution_count: i32,
    pub success_count: i32,
    pub failure_count: i32,
    pub trust_score: f64,
    pub computed_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub priority: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    pub skill_id: String,
    pub path: PathBuf,
    pub content: String,
    pub meta: SkillMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionAnalysis {
    pub skill_applied: bool,
    pub skill_helpful: bool,
    pub tool_calls: Vec<String>,
    pub error_category: Option<String>,
    pub improvement_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionRequest {
    pub evolution_type: String,
    pub target_skill_ids: Vec<i64>,
    pub category: Option<String>,
    pub direction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionResult {
    pub success: bool,
    pub skill_id: Option<i64>,
    pub evolution_type: String,
    pub message: String,
}

/// One row in the recent-evolution feed returned by
/// `GET /skills/evolution/recent`. `origin` is one of
/// `fixed` | `derived` | `captured`, sourced from `skill_tags`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionFeedRow {
    pub skill_id: i64,
    pub name: String,
    pub version: i32,
    pub origin: String,
    pub parent_ids: Vec<i64>,
    pub agent: String,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_cat() {
        assert_eq!(SkillCategory::ToolGuide.to_string(), "tool_guide");
    }
    #[test]
    fn test_origin() {
        for o in &[SkillOrigin::Imported, SkillOrigin::Fixed] {
            assert_eq!(&o.to_string().parse::<SkillOrigin>().unwrap(), o);
        }
    }
    #[test]
    fn test_evo() {
        for e in &[EvolutionType::Fix, EvolutionType::Captured] {
            assert_eq!(&e.to_string().parse::<EvolutionType>().unwrap(), e);
        }
    }
    #[test]
    fn test_stages() {
        assert_eq!(pipeline_stages().len(), 6);
    }
    #[test]
    fn test_vis() {
        assert_eq!(
            "private".parse::<SkillVisibility>().unwrap(),
            SkillVisibility::Private
        );
    }
}
