use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub is_code_query: bool,
    pub detected_language: Option<String>,
    pub recommended_mode: String,
    pub confidence: f64,
}

/// Analyze a query to detect if it's code-related and recommend search mode.
pub fn analyze_query(query: &str) -> RoutingDecision {
    let lower = query.to_lowercase();

    // Check for code patterns
    let code_indicators = [
        "function", "class", "struct", "enum", "impl", "def ", "return",
        "import", "require", "module", "package", "crate",
        "async", "await", "pub fn", "const ", "let ", "var ",
        "interface", "type ", "trait", "generic",
        "error", "bug", "fix", "compile", "build", "test",
        "api", "endpoint", "route", "handler", "middleware",
        "database", "query", "sql", "schema", "migration",
        "git", "commit", "branch", "merge", "deploy",
    ];

    let code_score: f64 = code_indicators.iter()
        .filter(|&&ind| lower.contains(ind))
        .count() as f64;

    // Check for language-specific patterns
    let detected_language = detect_language(query);

    // Check for code syntax markers
    let has_backticks = query.contains('`');
    let has_parens = query.contains("()") || query.contains("(");
    let has_brackets = query.contains("[]") || query.contains("{}");
    let has_arrow = query.contains("->") || query.contains("=>");
    let has_colons = query.contains("::");

    let syntax_score = [has_backticks, has_parens, has_brackets, has_arrow, has_colons]
        .iter().filter(|&&b| b).count() as f64;

    let total_score = code_score * 0.3 + syntax_score * 0.5;
    let is_code_query = total_score >= 1.0;

    let recommended_mode = if is_code_query {
        "fact_recall".to_string()  // Code queries benefit from precise vector matching
    } else if lower.contains("prefer") || lower.contains("like") || lower.contains("favorite") {
        "preference".to_string()
    } else if lower.contains("when") || lower.contains("last") || lower.contains("recent") || lower.contains("today") {
        "temporal".to_string()
    } else {
        "reasoning".to_string()
    };

    let confidence = (total_score / 3.0).clamp(0.1, 1.0);

    RoutingDecision {
        is_code_query,
        detected_language,
        recommended_mode,
        confidence,
    }
}

/// Simple programming language detection from query text.
fn detect_language(query: &str) -> Option<String> {
    let lower = query.to_lowercase();
    let patterns: &[(&str, &[&str])] = &[
        ("rust", &["rust", "cargo", "crate", "impl ", "fn ", "pub fn", "::new(", "unwrap"]),
        ("python", &["python", "pip", "django", "flask", "def ", "import ", "self."]),
        ("javascript", &["javascript", "npm", "node", "react", "const ", "let ", "=>"]),
        ("typescript", &["typescript", "tsx", "interface ", "type ", ": string", ": number"]),
        ("go", &["golang", "go ", "goroutine", "func ", "go mod"]),
        ("java", &["java", "maven", "gradle", "public class", "spring"]),
    ];

    for (lang, keywords) in patterns {
        let matches = keywords.iter().filter(|&&k| lower.contains(k)).count();
        if matches >= 2 {
            return Some(lang.to_string());
        }
    }

    None
}
