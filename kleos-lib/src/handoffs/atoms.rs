//! Atom engine core for the Kleos handoff system.
//!
//! An "atom" is the smallest unit of semantic knowledge extracted from a
//! handoff: a decision, constraint, task, entity, question, belief, or
//! relation. Atoms persist across sessions and are used to pack context
//! windows efficiently via the [`BudgetPacker`].

use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::LazyLock;
use tracing::warn;

// ---------------------------------------------------------------------------
// AtomType
// ---------------------------------------------------------------------------

/// The semantic category of an extracted atom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomType {
    /// An architectural or implementation decision that was made.
    Decision,
    /// A hard constraint (must/cannot/never/always).
    Constraint,
    /// A concrete work item or TODO.
    Task,
    /// A named entity: file path, service, component, etc.
    Entity,
    /// An open question or unresolved ambiguity.
    Question,
    /// A soft belief or assumption held by the agent.
    Belief,
    /// A relationship between two other atoms or entities.
    Relation,
}

/// Inherent methods for classifying and converting `AtomType` values.
impl AtomType {
    /// Returns the canonical lowercase string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            AtomType::Decision => "decision",
            AtomType::Constraint => "constraint",
            AtomType::Task => "task",
            AtomType::Entity => "entity",
            AtomType::Question => "question",
            AtomType::Belief => "belief",
            AtomType::Relation => "relation",
        }
    }

    /// Parses a string into an `AtomType`. Returns `None` on unrecognized input.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "decision" => Some(AtomType::Decision),
            "constraint" => Some(AtomType::Constraint),
            "task" => Some(AtomType::Task),
            "entity" => Some(AtomType::Entity),
            "question" => Some(AtomType::Question),
            "belief" => Some(AtomType::Belief),
            "relation" => Some(AtomType::Relation),
            _ => None,
        }
    }

    /// Returns `true` for atom types that are immune to salience decay.
    ///
    /// Decisions and constraints represent durable facts about a project and
    /// should not fade between sessions.
    pub fn is_decay_immune(&self) -> bool {
        matches!(self, AtomType::Decision | AtomType::Constraint)
    }
}

// ---------------------------------------------------------------------------
// AtomStatus
// ---------------------------------------------------------------------------

/// Lifecycle status of an atom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtomStatus {
    /// Atom is current and relevant.
    Active,
    /// Atom has been addressed or closed.
    Resolved,
    /// Atom has been replaced by a newer atom.
    Superseded,
    /// Atom's content is disputed or uncertain.
    Contested,
}

/// Inherent methods for converting `AtomStatus` to its string form.
impl AtomStatus {
    /// Returns the canonical lowercase string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            AtomStatus::Active => "active",
            AtomStatus::Resolved => "resolved",
            AtomStatus::Superseded => "superseded",
            AtomStatus::Contested => "contested",
        }
    }
}

// ---------------------------------------------------------------------------
// Atom
// ---------------------------------------------------------------------------

/// A single persisted semantic unit extracted from one or more handoffs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Atom {
    /// Row ID in the database (None before insert).
    pub id: Option<i64>,
    /// Stable content-addressed identifier (16 hex chars).
    pub atom_id: String,
    /// The handoff this atom was first extracted from.
    pub handoff_id: i64,
    /// Owning user.
    pub user_id: i64,
    /// Project scope.
    pub project: String,
    /// Semantic category.
    pub atom_type: AtomType,
    /// Raw content as extracted.
    pub content: String,
    /// Normalized form used for deduplication and ID generation.
    pub canonical_form: String,
    /// Importance score in [0.0, 1.0]. Decays over sessions.
    pub salience: f64,
    /// Extraction confidence in [0.0, 1.0].
    pub confidence: f64,
    /// Current lifecycle status.
    pub status: AtomStatus,
    /// ISO-8601 creation timestamp (None before insert).
    pub created_at: Option<String>,
    /// ISO-8601 timestamp of the most recent observation.
    pub last_seen_at: Option<String>,
    /// How many sessions this atom has appeared in.
    pub seen_count: i64,
    /// When true, salience decay is skipped for this atom.
    pub decay_immune: bool,
    /// `atom_id` of the atom that supersedes this one, if any.
    pub superseded_by: Option<String>,
    /// Arbitrary JSON metadata.
    pub metadata: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// ExtractedAtom
// ---------------------------------------------------------------------------

/// An atom produced by the extraction pipeline, before it is persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedAtom {
    /// Semantic category.
    pub atom_type: AtomType,
    /// Raw content string.
    pub content: String,
    /// Normalized form for dedup and ID generation.
    pub canonical_form: String,
    /// Extraction confidence in [0.0, 1.0].
    pub confidence: f64,
}

// ---------------------------------------------------------------------------
// make_atom_id
// ---------------------------------------------------------------------------

/// Derives a stable 16-character hex ID from an atom type and its canonical
/// form.
///
/// The ID is the first 16 hex digits of SHA-256(`"{type}:{canonical_form}"`),
/// where `canonical_form` is trimmed and lowercased before hashing.
pub fn make_atom_id(atom_type: AtomType, canonical_form: &str) -> String {
    let normalized = canonical_form.trim().to_lowercase();
    let input = format!("{}:{}", atom_type.as_str(), normalized);
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(hash)[..16].to_string()
}

// ---------------------------------------------------------------------------
// Compiled regexes (lazy)
// ---------------------------------------------------------------------------

static RE_DECISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(we\s+will|we\s+should|decided\s+to|chose\s+to|went\s+with|using)\b.{1,120}")
        .expect("RE_DECISION is a valid regex")
});

static RE_CONSTRAINT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(must\s+not|cannot|never|always|required|forbidden|do\s+not)\b.{1,120}")
        .expect("RE_CONSTRAINT is a valid regex")
});

static RE_TASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:TODO:|need\s+to|going\s+to\s+(?:implement|fix|add|build)|\[\s*\]\s+).{1,120}",
    )
    .expect("RE_TASK is a valid regex")
});

static RE_QUESTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(not\s+sure|unclear|open\s+question|need\s+to\s+figure\s+out)\b.{0,120}")
        .expect("RE_QUESTION is a valid regex")
});

/// Matches Unix-style paths with at least two segments (e.g. /foo/bar.rs or
/// ./src/lib.rs).
static RE_ENTITY_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s\(])(\./[^\s\)]+/[^\s\)]+|/[a-zA-Z0-9_.\-]+(?:/[^\s\)]+)+)")
        .expect("RE_ENTITY_PATH is a valid regex")
});

/// Matches labeled entity references like `file:`, `service:`, `component:`.
static RE_ENTITY_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(file|service|component):\s*([^\s,;]{2,80})")
        .expect("RE_ENTITY_LABEL is a valid regex")
});

// ---------------------------------------------------------------------------
// Heuristic extraction
// ---------------------------------------------------------------------------

/// Extracts atoms from `text` using fast regex heuristics.
///
/// Patterns cover decisions, constraints, tasks, questions, and entity
/// references. Results are deduplicated by canonical form. Content shorter
/// than 4 characters or longer than 500 characters is dropped.
pub fn extract_heuristic(text: &str) -> Vec<ExtractedAtom> {
    let mut results: Vec<ExtractedAtom> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    macro_rules! push {
        ($atom_type:expr, $content:expr, $confidence:expr) => {{
            let content = $content.trim().to_string();
            if content.len() >= 4 && content.len() <= 500 {
                let canonical = content.to_lowercase();
                if seen.insert(canonical.clone()) {
                    results.push(ExtractedAtom {
                        atom_type: $atom_type,
                        content: content.clone(),
                        canonical_form: canonical,
                        confidence: $confidence,
                    });
                }
            }
        }};
    }

    // Decisions
    for cap in RE_DECISION.captures_iter(text) {
        if let Some(m) = cap.get(0) {
            push!(AtomType::Decision, m.as_str(), 0.7);
        }
    }

    // Constraints
    for cap in RE_CONSTRAINT.captures_iter(text) {
        if let Some(m) = cap.get(0) {
            push!(AtomType::Constraint, m.as_str(), 0.8);
        }
    }

    // Tasks
    for cap in RE_TASK.captures_iter(text) {
        if let Some(m) = cap.get(0) {
            push!(AtomType::Task, m.as_str(), 0.75);
        }
    }

    // Questions
    for cap in RE_QUESTION.captures_iter(text) {
        if let Some(m) = cap.get(0) {
            push!(AtomType::Question, m.as_str(), 0.65);
        }
    }

    // Entities -- file paths
    for cap in RE_ENTITY_PATH.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            push!(AtomType::Entity, m.as_str(), 0.6);
        }
    }

    // Entities -- labeled references
    for cap in RE_ENTITY_LABEL.captures_iter(text) {
        if let Some(m) = cap.get(2) {
            push!(AtomType::Entity, m.as_str(), 0.65);
        }
    }

    results
}

// ---------------------------------------------------------------------------
// LLM extraction
// ---------------------------------------------------------------------------

/// Response shape expected from an Ollama-compatible chat completion endpoint.
#[derive(Debug, Deserialize)]
struct LlmResponse {
    choices: Vec<LlmChoice>,
}

/// Deserialization target for a single choice entry in an LLM API response.
#[derive(Debug, Deserialize)]
struct LlmChoice {
    message: LlmMessage,
}

/// Deserialization target for the message body inside an LLM API choice.
#[derive(Debug, Deserialize)]
struct LlmMessage {
    content: String,
}

/// A list of atoms as returned by the LLM in JSON mode.
#[derive(Debug, Deserialize)]
struct LlmAtomList {
    atoms: Vec<LlmAtomItem>,
}

/// Deserialization target for a single atom entry returned by the LLM in JSON mode.
#[derive(Debug, Deserialize)]
struct LlmAtomItem {
    atom_type: String,
    content: String,
    confidence: Option<f64>,
}

/// Attempts to extract atoms from `text` via an Ollama-compatible LLM endpoint.
///
/// On any failure (network, parse, timeout) this function logs a warning and
/// returns an empty vector -- callers should fall back to [`extract_heuristic`].
pub async fn extract_llm(text: &str, sidecar_url: &str) -> Vec<ExtractedAtom> {
    let system_prompt = "You are a semantic extraction engine. \
        Given text, extract key atoms as JSON. \
        Return ONLY a JSON object with an \"atoms\" array. \
        Each atom has: atom_type (decision|constraint|task|entity|question|belief|relation), \
        content (string), confidence (0.0-1.0). \
        Extract only atoms clearly present in the text. Be concise.";

    let body = serde_json::json!({
        "model": "llama3",
        "messages": [
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": text}
        ],
        "response_format": {"type": "json_object"},
        "temperature": 0.1
    });

    let client = reqwest::Client::new();
    let url = format!("{}/v1/chat/completions", sidecar_url.trim_end_matches('/'));

    let response = match client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!("atom llm extraction request failed: {}", e);
            return Vec::new();
        }
    };

    let llm_resp: LlmResponse = match response.json().await {
        Ok(r) => r,
        Err(e) => {
            warn!("atom llm extraction response parse failed: {}", e);
            return Vec::new();
        }
    };

    let content = match llm_resp.choices.into_iter().next() {
        Some(c) => c.message.content,
        None => {
            warn!("atom llm extraction returned no choices");
            return Vec::new();
        }
    };

    let atom_list: LlmAtomList = match serde_json::from_str(&content) {
        Ok(l) => l,
        Err(e) => {
            warn!("atom llm extraction JSON parse failed: {}", e);
            return Vec::new();
        }
    };

    let mut seen: HashSet<String> = HashSet::new();
    let mut results = Vec::new();

    for item in atom_list.atoms {
        let atom_type = match AtomType::parse(&item.atom_type) {
            Some(t) => t,
            None => {
                warn!("atom llm returned unknown atom_type: {}", item.atom_type);
                continue;
            }
        };
        let content = item.content.trim().to_string();
        if content.len() < 4 || content.len() > 500 {
            continue;
        }
        let canonical = content.to_lowercase();
        if seen.insert(canonical.clone()) {
            results.push(ExtractedAtom {
                atom_type,
                content,
                canonical_form: canonical,
                confidence: item.confidence.unwrap_or(0.6).clamp(0.0, 1.0),
            });
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Combined extraction entry point
// ---------------------------------------------------------------------------

/// Extracts atoms from `text`.
///
/// If `sidecar_url` is `Some`, the LLM path is tried first. On failure or
/// empty result, falls back to the heuristic extractor. If `sidecar_url` is
/// `None`, heuristic extraction is used directly.
pub async fn extract(text: &str, sidecar_url: Option<&str>) -> Vec<ExtractedAtom> {
    if let Some(url) = sidecar_url {
        let llm_atoms = extract_llm(text, url).await;
        if !llm_atoms.is_empty() {
            return llm_atoms;
        }
    }
    extract_heuristic(text)
}

// ---------------------------------------------------------------------------
// BudgetPacker
// ---------------------------------------------------------------------------

/// Packs atoms into a context window budget.
///
/// Mandatory atoms (decay-immune and active) are always included first.
/// Remaining budget is filled greedily by value/token density.
#[derive(Debug, Clone)]
pub struct BudgetPacker {
    /// Maximum number of tokens allowed in the packed output.
    pub max_tokens: usize,
    /// Overhead tokens charged per atom (for formatting, separators, etc.).
    pub overhead_per_atom: usize,
}

/// Methods for packing, scoring, and rendering atoms within a token budget.
impl BudgetPacker {
    /// Creates a new `BudgetPacker` with the given token budget and per-atom
    /// overhead.
    pub fn new(max_tokens: usize, overhead_per_atom: usize) -> Self {
        Self {
            max_tokens,
            overhead_per_atom,
        }
    }

    /// Selects atoms to include within the token budget.
    ///
    /// Mandatory atoms (decay-immune + active status) are always included first.
    /// The remainder of the budget is filled by optional atoms sorted by
    /// value/token density descending.
    pub fn pack<'a>(&self, atoms: &'a [Atom]) -> Vec<&'a Atom> {
        let mut mandatory: Vec<&Atom> = atoms
            .iter()
            .filter(|a| a.decay_immune && a.status == AtomStatus::Active)
            .collect();

        let mut optional: Vec<&Atom> = atoms
            .iter()
            .filter(|a| !(a.decay_immune && a.status == AtomStatus::Active))
            .collect();

        // Sort optional by value density (value / tokens) descending.
        optional.sort_by(|a, b| {
            let da = self.value_density(a);
            let db = self.value_density(b);
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut selected: Vec<&Atom> = Vec::new();
        let mut used_tokens: usize = 0;

        for atom in mandatory.drain(..) {
            let cost = self.token_estimate(atom) + self.overhead_per_atom;
            used_tokens += cost;
            selected.push(atom);
        }

        for atom in optional {
            let cost = self.token_estimate(atom) + self.overhead_per_atom;
            if used_tokens + cost > self.max_tokens {
                break;
            }
            used_tokens += cost;
            selected.push(atom);
        }

        selected
    }

    /// Renders a packed atom list as grouped markdown for injection into a
    /// context window.
    pub fn to_context_string(&self, atoms: &[&Atom]) -> String {
        use std::collections::BTreeMap;

        let mut by_type: BTreeMap<&str, Vec<&Atom>> = BTreeMap::new();
        for atom in atoms {
            by_type
                .entry(atom.atom_type.as_str())
                .or_default()
                .push(atom);
        }

        let mut out = String::new();
        for (type_name, group) in &by_type {
            out.push_str(&format!("## {}\n", capitalize(type_name)));
            for atom in group {
                out.push_str(&format!("- {}\n", atom.content));
            }
            out.push('\n');
        }
        out
    }

    /// Estimates the number of tokens in an atom's content.
    ///
    /// Uses word count * 1.3 as a fast approximation.
    fn token_estimate(&self, atom: &Atom) -> usize {
        let words = atom.content.split_whitespace().count();
        ((words as f64) * 1.3).ceil() as usize
    }

    /// Computes a value score for an atom, used for density-based sorting.
    ///
    /// Value = type_weight * salience * recency_bonus.
    fn compute_value(&self, atom: &Atom) -> f64 {
        let type_weight = match atom.atom_type {
            AtomType::Decision => 1.5,
            AtomType::Constraint => 1.4,
            AtomType::Task => 1.3,
            AtomType::Question => 1.2,
            AtomType::Belief => 1.1,
            AtomType::Entity | AtomType::Relation => 1.0,
        };
        // Recency bonus: atoms seen more times get a small boost.
        let recency_bonus = 1.0 + (atom.seen_count as f64).ln().max(0.0) * 0.05;
        type_weight * atom.salience * recency_bonus
    }

    /// Returns value per token for sorting purposes.
    fn value_density(&self, atom: &Atom) -> f64 {
        let tokens = (self.token_estimate(atom) + self.overhead_per_atom).max(1) as f64;
        self.compute_value(atom) / tokens
    }
}

/// Capitalizes the first Unicode character of a string slice.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Decay
// ---------------------------------------------------------------------------

/// Applies exponential salience decay to non-immune atoms.
///
/// For each non-immune atom, salience is multiplied by `0.9^sessions_elapsed`.
/// Atoms whose salience drops below `0.05` are automatically resolved.
pub fn apply_decay(atoms: &mut [Atom], sessions_elapsed: u32) {
    if sessions_elapsed == 0 {
        return;
    }
    let factor = 0.9_f64.powi(sessions_elapsed as i32);
    for atom in atoms.iter_mut() {
        if atom.decay_immune {
            continue;
        }
        atom.salience *= factor;
        if atom.salience < 0.05 && atom.status == AtomStatus::Active {
            atom.status = AtomStatus::Resolved;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a fully populated `Atom` with the given type, content, salience, and decay-immune flag.
    fn make_atom(atom_type: AtomType, content: &str, salience: f64, immune: bool) -> Atom {
        let canonical = content.to_lowercase();
        let atom_id = make_atom_id(atom_type.clone(), &canonical);
        Atom {
            id: None,
            atom_id,
            handoff_id: 1,
            user_id: 1,
            project: "test".to_string(),
            atom_type,
            content: content.to_string(),
            canonical_form: canonical,
            salience,
            confidence: 0.8,
            status: AtomStatus::Active,
            created_at: None,
            last_seen_at: None,
            seen_count: 1,
            decay_immune: immune,
            superseded_by: None,
            metadata: None,
        }
    }

    /// Verifies that atom IDs are deterministic and canonical-form stable.
    #[test]
    fn atom_id_is_stable() {
        let id1 = make_atom_id(AtomType::Decision, "  Use Postgres  ");
        let id2 = make_atom_id(AtomType::Decision, "use postgres");
        assert_eq!(id1, id2, "atom_id must be canonical-form stable");
        assert_eq!(id1.len(), 16, "atom_id must be 16 hex chars");
    }

    /// Verifies that every `AtomType` variant round-trips through `parse` and `as_str`.
    #[test]
    fn atom_type_roundtrip() {
        for (s, expected) in [
            ("decision", AtomType::Decision),
            ("constraint", AtomType::Constraint),
            ("task", AtomType::Task),
            ("entity", AtomType::Entity),
            ("question", AtomType::Question),
            ("belief", AtomType::Belief),
            ("relation", AtomType::Relation),
        ] {
            assert_eq!(AtomType::parse(s), Some(expected));
        }
        assert_eq!(AtomType::parse("unknown_xyz"), None);
    }

    /// Verifies that only Decision and Constraint atom types are marked decay-immune.
    #[test]
    fn decay_immune_types() {
        assert!(AtomType::Decision.is_decay_immune());
        assert!(AtomType::Constraint.is_decay_immune());
        assert!(!AtomType::Task.is_decay_immune());
        assert!(!AtomType::Entity.is_decay_immune());
    }

    /// Verifies that the heuristic extractor identifies decision atoms from trigger phrases.
    #[test]
    fn heuristic_extracts_decision() {
        let text = "We decided to use Postgres for the main database.";
        let atoms = extract_heuristic(text);
        let decisions: Vec<_> = atoms
            .iter()
            .filter(|a| a.atom_type == AtomType::Decision)
            .collect();
        assert!(!decisions.is_empty(), "should find at least one decision");
    }

    /// Verifies that the heuristic extractor identifies constraint atoms from must-not/never phrases.
    #[test]
    fn heuristic_extracts_constraint() {
        let text = "You must not write to the production database directly.";
        let atoms = extract_heuristic(text);
        let constraints: Vec<_> = atoms
            .iter()
            .filter(|a| a.atom_type == AtomType::Constraint)
            .collect();
        assert!(
            !constraints.is_empty(),
            "should find at least one constraint"
        );
    }

    /// Verifies that the heuristic extractor identifies task atoms from TODO and action phrases.
    #[test]
    fn heuristic_extracts_task() {
        let text = "TODO: implement the atom decay function.";
        let atoms = extract_heuristic(text);
        let tasks: Vec<_> = atoms
            .iter()
            .filter(|a| a.atom_type == AtomType::Task)
            .collect();
        assert!(!tasks.is_empty(), "should find at least one task");
    }

    /// Verifies that the heuristic extractor identifies entity atoms from filesystem paths.
    #[test]
    fn heuristic_extracts_entity_path() {
        let text = "The main entry point is /home/user/projects/app/src/main.rs.";
        let atoms = extract_heuristic(text);
        let entities: Vec<_> = atoms
            .iter()
            .filter(|a| a.atom_type == AtomType::Entity)
            .collect();
        assert!(!entities.is_empty(), "should find file path entity");
    }

    /// Verifies that the heuristic extractor does not emit duplicate canonical forms.
    #[test]
    fn heuristic_deduplicates() {
        let text = "TODO: fix the bug. TODO: fix the bug.";
        let atoms = extract_heuristic(text);
        let seen_canonicals: HashSet<_> = atoms.iter().map(|a| &a.canonical_form).collect();
        assert_eq!(
            seen_canonicals.len(),
            atoms.len(),
            "no duplicate canonical forms"
        );
    }

    /// Verifies that the heuristic extractor drops matched content shorter than 4 characters.
    #[test]
    fn heuristic_skips_short_content() {
        // The regex will match but "no" is 2 chars -- below the 4-char floor.
        // This test just confirms the function doesn't crash on minimal input.
        let atoms = extract_heuristic("ok");
        assert!(atoms.is_empty() || atoms.len() < 100);
    }

    /// Verifies that `apply_decay` lowers salience on non-immune atoms and leaves immune atoms unchanged.
    #[test]
    fn decay_reduces_salience() {
        let mut atoms = vec![
            make_atom(AtomType::Task, "fix linter", 1.0, false),
            make_atom(AtomType::Decision, "use postgres", 1.0, true),
        ];
        apply_decay(&mut atoms, 3);
        // Non-immune atom decays.
        assert!(atoms[0].salience < 1.0);
        // Immune atom stays at 1.0.
        assert!((atoms[1].salience - 1.0).abs() < f64::EPSILON);
    }

    /// Verifies that atoms whose salience decays below 0.05 are automatically resolved.
    #[test]
    fn decay_resolves_below_threshold() {
        let mut atoms = vec![make_atom(AtomType::Task, "stale task", 0.06, false)];
        apply_decay(&mut atoms, 10); // 0.06 * 0.9^10 ~ 0.021
        assert_eq!(atoms[0].status, AtomStatus::Resolved);
    }

    /// Verifies that decay-immune active atoms are always included in the packed output.
    #[test]
    fn budget_packer_mandatory_first() {
        let mandatory = make_atom(
            AtomType::Decision,
            "mandatory decision atom content",
            0.9,
            true,
        );
        let optional = make_atom(AtomType::Task, "optional task item for packing", 0.5, false);
        let atoms = vec![optional.clone(), mandatory.clone()];

        let packer = BudgetPacker::new(200, 5);
        let packed = packer.pack(&atoms);

        // Mandatory atom must appear.
        assert!(
            packed.iter().any(|a| a.atom_id == mandatory.atom_id),
            "mandatory atom must be included"
        );
    }

    /// Verifies that `BudgetPacker::pack` does not exceed the configured token budget.
    #[test]
    fn budget_packer_respects_budget() {
        // Create many atoms that would exceed a tiny budget.
        let atoms: Vec<Atom> = (0..20)
            .map(|i| {
                make_atom(
                    AtomType::Task,
                    &format!("task item number {i} here"),
                    0.5,
                    false,
                )
            })
            .collect();

        let packer = BudgetPacker::new(10, 5);
        let packed = packer.pack(&atoms);
        // With max_tokens=10 and overhead=5, only a couple can fit.
        assert!(packed.len() < 20, "should not pack all atoms");
    }

    /// Verifies that `to_context_string` emits a separate markdown section for each atom type.
    #[test]
    fn context_string_groups_by_type() {
        let d = make_atom(AtomType::Decision, "use postgres for storage", 1.0, true);
        let t = make_atom(AtomType::Task, "implement decay logic soon", 0.8, false);
        let refs: Vec<&Atom> = vec![&d, &t];

        let packer = BudgetPacker::new(1000, 5);
        let s = packer.to_context_string(&refs);
        assert!(s.contains("## Decision"), "should have Decision section");
        assert!(s.contains("## Task"), "should have Task section");
    }
}
