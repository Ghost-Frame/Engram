//! EN-syntax parser. Lenient by design: any line that contains at least a
//! subject (the leading word) is accepted, even if `do:`/`needs:`/`yields:`
//! are missing. Unknown keywords are ignored so the parser stays forward
//! compatible.

use serde::{Deserialize, Serialize};

/// One parsed EN statement. Field names match the on-the-wire JSON shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnStatement {
    pub subject: String,
    pub action: Option<String>,
    pub needs: Vec<String>,
    pub yields: Vec<String>,
}

/// Split a comma list, trim whitespace, drop empties.
fn split_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Split `Subject do: ACTION needs: A,B yields: C,D` into the four fields.
/// Empty subject yields `None`.
fn parse_one(stmt: &str) -> Option<EnStatement> {
    let raw = stmt.trim();
    if raw.is_empty() {
        return None;
    }

    // Walk through the input collecting (keyword, value) spans. Anything
    // before the first keyword is the subject.
    let lower = raw.to_lowercase();
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    for kw in ["do:", "needs:", "yields:"] {
        let mut start = 0usize;
        while let Some(rel) = lower[start..].find(kw) {
            let pos = start + rel;
            // Treat as keyword only when preceded by whitespace or BOL.
            let before_ok = pos == 0 || lower.as_bytes()[pos - 1].is_ascii_whitespace();
            if before_ok {
                spans.push((pos, pos + kw.len(), kw));
            }
            start = pos + kw.len();
        }
    }
    spans.sort_by_key(|(s, _, _)| *s);

    let subject_end = spans.first().map(|(s, _, _)| *s).unwrap_or(raw.len());
    let subject = raw[..subject_end].trim().to_string();
    if subject.is_empty() {
        return None;
    }

    let mut out = EnStatement {
        subject,
        action: None,
        needs: Vec::new(),
        yields: Vec::new(),
    };

    for (i, (_, val_start, kw)) in spans.iter().enumerate() {
        let val_end = spans
            .get(i + 1)
            .map(|(next_start, _, _)| *next_start)
            .unwrap_or(raw.len());
        let value = raw[*val_start..val_end].trim().trim_end_matches('.').trim();
        match *kw {
            "do:" => out.action = Some(value.to_string()).filter(|s| !s.is_empty()),
            "needs:" => out.needs = split_list(value),
            "yields:" => out.yields = split_list(value),
            _ => {}
        }
    }

    Some(out)
}

/// Parse a multi-statement EN source string into individual statements.
/// Statements are separated by periods (`.`); newlines are treated as
/// whitespace. Each subject must be unique on the input -- duplicate
/// subjects merge their `needs:` and `yields:` lists so a single node can
/// be described across multiple lines.
pub fn parse_en_source(source: &str) -> Vec<EnStatement> {
    // Normalise newlines to whitespace, then split on '.' to get statements.
    let normalised = source.replace(['\n', '\r'], " ");
    let mut by_subject: std::collections::BTreeMap<String, EnStatement> = Default::default();
    let mut order: Vec<String> = Vec::new();

    for raw in normalised.split('.') {
        if let Some(stmt) = parse_one(raw) {
            let key = stmt.subject.clone();
            match by_subject.get_mut(&key) {
                Some(existing) => {
                    if existing.action.is_none() {
                        existing.action = stmt.action.clone();
                    }
                    for n in stmt.needs {
                        if !existing.needs.contains(&n) {
                            existing.needs.push(n);
                        }
                    }
                    for y in stmt.yields {
                        if !existing.yields.contains(&y) {
                            existing.yields.push(y);
                        }
                    }
                }
                None => {
                    order.push(key.clone());
                    by_subject.insert(key, stmt);
                }
            }
        }
    }

    order
        .into_iter()
        .filter_map(|k| by_subject.remove(&k))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_statement() {
        let stmts = parse_en_source("API do: serve needs: DB yields: JSON.");
        assert_eq!(stmts.len(), 1);
        let s = &stmts[0];
        assert_eq!(s.subject, "API");
        assert_eq!(s.action.as_deref(), Some("serve"));
        assert_eq!(s.needs, vec!["DB".to_string()]);
        assert_eq!(s.yields, vec!["JSON".to_string()]);
    }

    #[test]
    fn comma_lists_are_split() {
        let stmts = parse_en_source("Pipeline do: process needs: a, b , c yields: x ,y.");
        assert_eq!(stmts[0].needs, vec!["a", "b", "c"]);
        assert_eq!(stmts[0].yields, vec!["x", "y"]);
    }

    #[test]
    fn multiline_chains() {
        let src =
            "API do: serve needs: DB yields: JSON.\nUI do: render needs: JSON yields: pixels.";
        let stmts = parse_en_source(src);
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[1].subject, "UI");
    }

    #[test]
    fn duplicate_subjects_merge() {
        let src = "X needs: a. X needs: b yields: c.";
        let stmts = parse_en_source(src);
        assert_eq!(stmts.len(), 1);
        assert_eq!(stmts[0].needs, vec!["a", "b"]);
        assert_eq!(stmts[0].yields, vec!["c"]);
    }

    #[test]
    fn missing_keywords_tolerated() {
        let stmts = parse_en_source("Source yields: data.");
        assert_eq!(stmts[0].subject, "Source");
        assert!(stmts[0].action.is_none());
        assert!(stmts[0].needs.is_empty());
        assert_eq!(stmts[0].yields, vec!["data"]);
    }

    #[test]
    fn empty_source_returns_empty() {
        assert!(parse_en_source("").is_empty());
        assert!(parse_en_source("   .   .  ").is_empty());
    }
}
