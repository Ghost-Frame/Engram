//! Request and query-parameter structs for the forge route family.
//!
//! kleos-lib exposes `ApproachItem` as `pub` so it is re-exported here.
//! All other input shapes are defined here and mapped to lib fn parameters
//! in the handler.
use serde::Deserialize;

pub use kleos_lib::forge::approaches::ApproachItem;

/// Body for `POST /forge/spec-task`.
///
/// Maps 1:1 to `kleos_lib::forge::spec::spec_task` positional parameters.
#[derive(Deserialize)]
pub struct SpecTaskBody {
    /// Optional agent session identifier; used by the gate enforcement query.
    pub session_id: Option<String>,
    /// Human-readable description of the task.
    pub task_description: String,
    /// Category of work (feature, bugfix, refactor, enhancement, test, docs).
    pub task_type: String,
    /// List of observable outcomes that define done (minimum 2).
    pub acceptance_criteria: Vec<String>,
    /// Stable public interface description for the change.
    pub interface_contract: String,
    /// Known failure modes and boundary conditions (minimum 3).
    pub edge_cases: Vec<String>,
    /// Paths the agent expects to write; drives gate enforcement. Optional.
    pub files_to_touch: Option<Vec<String>>,
    /// Free-text external dependencies or blockers. Optional.
    pub dependencies: Option<String>,
}

/// Body for `POST /forge/update-spec`.
#[derive(Deserialize)]
pub struct UpdateSpecBody {
    /// ID of the spec to transition.
    pub spec_id: String,
    /// New lifecycle status (active, completed, failed, blocked).
    pub status: String,
    /// Optional human note recorded alongside the transition.
    pub note: Option<String>,
}

/// Query params for `GET /forge/specs`.
#[derive(Deserialize)]
pub struct ListSpecsQuery {
    /// Filter by lifecycle status.
    pub status: Option<String>,
    /// Maximum rows to return (default 20).
    pub limit: Option<usize>,
}

// Path parameter for `GET /forge/spec/{id}`: Axum extracts this via
// `Path<String>` directly; no wrapper struct is needed.

/// Body for `POST /forge/log-hypothesis`.
#[derive(Deserialize)]
pub struct LogHypothesisBody {
    /// Optional agent session identifier.
    pub session_id: Option<String>,
    /// Description of the observed bug or failure.
    pub bug_description: String,
    /// Agent's proposed root cause.
    pub hypothesis: String,
    /// Subjective confidence in [0.0, 1.0].
    pub confidence: Option<f64>,
    /// Optional parent spec ID to link to.
    pub spec_id: Option<String>,
}

/// Body for `POST /forge/log-outcome`.
#[derive(Deserialize)]
pub struct LogOutcomeBody {
    /// ID of the hypothesis to close.
    pub hypothesis_id: String,
    /// Verdict: correct, incorrect, or partial.
    pub outcome: String,
    /// Optional free-text notes about what was learned.
    pub notes: Option<String>,
}

/// Query params for `GET /forge/recall-errors`.
#[derive(Deserialize)]
pub struct RecallErrorsQuery {
    /// Keyword to search in bug_description and hypothesis columns.
    pub query: Option<String>,
    /// Maximum rows to return (default 10).
    pub limit: Option<usize>,
}

/// Body for `POST /forge/consider-approaches`.
#[derive(Deserialize)]
pub struct ConsiderApproachesBody {
    /// Optional parent spec ID to link the approaches to.
    pub spec_id: Option<String>,
    /// Short description of the design problem being evaluated.
    pub problem: String,
    /// The design alternatives (minimum 2).
    pub approaches: Vec<ApproachItem>,
    /// Zero-based index of the chosen alternative, if decided.
    pub chosen_index: Option<usize>,
}

/// Body for `POST /forge/verify`.
///
/// The client runs the command; this body carries the result for persistence.
#[derive(Deserialize)]
pub struct VerifyBody {
    /// Optional parent spec ID to link the record to.
    pub spec_id: Option<String>,
    /// The command string that was executed client-side.
    pub command: String,
    /// Process exit code.
    pub exit_code: i32,
    /// Whether the run is considered a success.
    pub success: bool,
    /// Wall-clock duration of the command in milliseconds.
    pub duration_ms: Option<i64>,
    /// Zero-based index into the spec's `acceptance_criteria` array.
    pub criteria_index: Option<i64>,
    /// First 4096 bytes of standard output (pre-clip client-side).
    pub stdout: Option<String>,
    /// First 4096 bytes of standard error (pre-clip client-side).
    pub stderr: Option<String>,
}

/// Body for `POST /forge/session-learn`.
#[derive(Deserialize)]
pub struct SessionLearnBody {
    /// Discovery text to persist.
    pub discovery: String,
    /// Optional surrounding context (file name, function, etc.).
    pub context: Option<String>,
    /// Optional tag list for future recall filtering.
    pub tags: Option<Vec<String>>,
    /// Optional parent spec ID.
    pub spec_id: Option<String>,
}

/// Query params for `GET /forge/session-recall`.
#[derive(Deserialize)]
pub struct SessionRecallQuery {
    /// Keyword to search in the `discovery` column.
    pub query: Option<String>,
    /// Maximum rows to return (default 10).
    pub limit: Option<usize>,
}

/// Body for `POST /forge/think`.
///
/// Passed directly to `agent_forge::tools::think::think`.
#[derive(Deserialize)]
pub struct ThinkBody {
    /// The problem statement to reason about.
    pub problem: Option<String>,
    /// Optional list of constraints that bound the solution space.
    pub constraints: Option<Vec<String>>,
    /// Optional context the caller already holds.
    pub context: Option<String>,
}

/// Body for `POST /forge/declare-unknowns`.
///
/// Passed directly to `agent_forge::tools::think::declare_unknowns`.
#[derive(Deserialize)]
pub struct DeclareUnknownsBody {
    /// One or more unknowns to declare. Must not be empty.
    pub unknowns: Option<Vec<UnknownItemBody>>,
}

/// One entry in a `declare-unknowns` request.
#[derive(Deserialize)]
pub struct UnknownItemBody {
    /// Short description of what is not yet known.
    pub description: String,
    /// When true, this unknown blocks forward progress.
    pub blocking: bool,
    /// Optional hint for how to resolve the unknown.
    pub resolution_hint: Option<String>,
}

/// Body for `POST /forge/comment-check` and `POST /forge/challenge-code`.
///
/// Exactly one of `path` or `content` must be supplied.
/// - `path`: server-visible absolute path; must resolve within `KLEOS_FORGE_FS_ROOTS`.
/// - `content`: raw source text; the server writes it to a temp file before scanning.
#[derive(Deserialize)]
pub struct FileOrContentBody {
    /// Absolute path on the server filesystem to the file to scan.
    pub path: Option<String>,
    /// Inline source content to scan (written to a temp file server-side).
    pub content: Option<String>,
    /// Optional file extension hint used when `content` is supplied (e.g. "rs").
    /// If omitted the temp file is written without an extension.
    pub extension: Option<String>,
}

/// Body for `POST /forge/repo-map`.
///
/// `path` must resolve within `KLEOS_FORGE_FS_ROOTS`.
#[derive(Deserialize)]
pub struct RepoMapBody {
    /// Absolute path to the directory root to scan.
    pub path: String,
    /// Optional list of path fragments to prioritise in the output.
    pub focus: Option<Vec<String>>,
    /// Token budget for the output (default 4000).
    pub max_tokens: Option<usize>,
}

/// Body for `POST /forge/search-code`.
///
/// `path` must resolve within `KLEOS_FORGE_FS_ROOTS`.
#[derive(Deserialize)]
pub struct SearchCodeBody {
    /// Symbol name fragment to search for (case-insensitive).
    pub query: Option<String>,
    /// Absolute path to the directory root to walk.
    pub path: String,
    /// Optional kind filter: "function", "class", "enum", etc.
    pub symbol_type: Option<String>,
    /// Maximum number of results to return (default 20).
    pub limit: Option<usize>,
}
