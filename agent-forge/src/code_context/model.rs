//! Public request and response contracts for deterministic code retrieval.

use serde::{Deserialize, Serialize};

/// A request for a bounded pack of code relevant to one natural-language task.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextQuery {
    /// Any path inside the Git repository to search.
    pub repo_root: String,
    /// Natural-language task or code-oriented search phrase.
    pub query: String,
    /// Approximate maximum number of model tokens in returned source text.
    #[serde(default = "default_context_tokens")]
    pub max_tokens: usize,
    /// Paths that should receive a strong deterministic ranking boost.
    #[serde(default)]
    pub focus_paths: Vec<String>,
    /// Paths recently touched in the current coding session.
    #[serde(default)]
    pub recent_paths: Vec<String>,
}

/// Return the conservative default context allowance.
fn default_context_tokens() -> usize {
    2_000
}

/// One exact source range selected for a context pack.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ContextSnippet {
    /// Repository-relative source path.
    pub path: String,
    /// One-based first source line.
    pub start_line: usize,
    /// One-based final source line.
    pub end_line: usize,
    /// Parser language selected from the file extension.
    pub language: String,
    /// Semantic unit kind such as function, type, import, or file.
    pub kind: String,
    /// Best available symbol name for the unit.
    pub symbol: String,
    /// Short explanation of the deterministic ranking signals.
    pub reason: String,
    /// Deterministic relevance score used for ordering.
    pub score: f64,
    /// Exact source text captured by the local index.
    pub text: String,
    /// SHA-256 hash of the file version that produced this snippet.
    pub content_hash: String,
}

/// A token-bounded code context result that may intentionally contain no snippets.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ContextPack {
    /// Ranked non-overlapping source snippets.
    pub snippets: Vec<ContextSnippet>,
    /// Conservative character-based token estimate for all snippets.
    pub estimated_tokens: usize,
    /// Whether otherwise-relevant snippets were omitted by the budget.
    pub truncated: bool,
    /// Monotonic revision of the repository index used for the query.
    pub index_revision: i64,
    /// Number of candidates rejected because the source file changed.
    pub stale_skipped: usize,
}

/// Rendering helpers for model-ready context packs.
impl ContextPack {
    /// Render snippets with explicit source ranges and selection reasons.
    pub fn render(&self) -> String {
        self.snippets
            .iter()
            .map(|snippet| {
                format!(
                    "### {}:{}-{} [{} {}]\nReason: {}\n```{}\n{}\n```",
                    snippet.path,
                    snippet.start_line,
                    snippet.end_line,
                    snippet.kind,
                    snippet.symbol,
                    snippet.reason,
                    snippet.language,
                    snippet.text.trim_end()
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// Direction filter for relation traversal.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationDirection {
    /// Return relations originating at the selected symbol.
    Outgoing,
    /// Return relations pointing to the selected symbol.
    Incoming,
    /// Return both incoming and outgoing relations.
    #[default]
    Both,
}

/// A bounded request for structural code relations.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RelationQuery {
    /// Any path inside the Git repository to inspect.
    pub repo_root: String,
    /// Optional exact or suffix symbol selector.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Optional repository-relative path selector.
    #[serde(default)]
    pub path: Option<String>,
    /// Optional relation-kind allowlist.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Incoming, outgoing, or bidirectional traversal.
    #[serde(default)]
    pub direction: RelationDirection,
    /// Maximum number of returned relations.
    #[serde(default = "default_relation_limit")]
    pub limit: usize,
}

/// Return the default relation result limit.
fn default_relation_limit() -> usize {
    50
}

/// One conservative relation inferred from the syntax tree.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CodeRelation {
    /// Relation kind such as calls, imports, contains, or test_of.
    pub kind: String,
    /// Direction relative to the selected symbol or path.
    pub direction: RelationDirection,
    /// Source symbol, or the containing file for top-level relations.
    pub source: String,
    /// Target symbol or imported module text.
    pub target: String,
    /// Repository-relative location of the source relation.
    pub path: String,
    /// One-based source line where the relation occurs.
    pub start_line: usize,
    /// One-based final line where the relation occurs.
    pub end_line: usize,
    /// Conservative syntactic confidence from zero to one.
    pub confidence: f64,
}

/// Statistics from one incremental repository refresh.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RefreshReport {
    /// Canonical repository root.
    pub repo_root: String,
    /// Monotonic repository index revision after the refresh.
    pub index_revision: i64,
    /// Supported source files encountered by the walker.
    pub files_seen: usize,
    /// New or content-changed files parsed and stored.
    pub files_indexed: usize,
    /// Previously indexed files no longer present.
    pub files_removed: usize,
    /// Files skipped because their content hash was unchanged.
    pub files_unchanged: usize,
    /// Supported files that could not be parsed as UTF-8 source.
    pub parse_failures: usize,
}

/// One compact symbol returned by compatibility map and search operations.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct IndexedSymbol {
    /// Repository-relative source path.
    pub path: String,
    /// One-based declaration line.
    pub line: usize,
    /// Semantic declaration kind.
    pub kind: String,
    /// Extracted symbol name.
    pub name: String,
    /// First source line of the declaration.
    pub signature: String,
}
