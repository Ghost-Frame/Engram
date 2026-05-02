pub use super::types::{SecretPattern, SecretPatternKind};

use std::future::Future;
use std::sync::OnceLock;

use regex::Regex;

use crate::Result;

fn secret_pattern_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\{\{(secret|secret-raw):([^/\}]+)/([^\}]+)\}\}")
            .expect("secret placeholder regex")
    })
}

pub fn has_secret_patterns(input: &str) -> bool {
    secret_pattern_regex().is_match(input)
}

pub fn find_secret_patterns(input: &str) -> Vec<SecretPattern> {
    secret_pattern_regex()
        .captures_iter(input)
        .filter_map(|caps| {
            let kind = match caps.get(1)?.as_str() {
                "secret" => SecretPatternKind::Secret,
                "secret-raw" => SecretPatternKind::Raw,
                _ => return None,
            };
            Some(SecretPattern {
                kind,
                service: caps.get(2)?.as_str().to_string(),
                key: caps.get(3)?.as_str().to_string(),
            })
        })
        .collect()
}

#[tracing::instrument(skip_all, fields(input_len = input.len()))]
pub async fn resolve_patterns<F, Fut>(input: &str, mut resolver: F) -> Result<String>
where
    F: FnMut(SecretPattern) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    for caps in secret_pattern_regex().captures_iter(input) {
        let whole = match caps.get(0) {
            Some(value) => value,
            None => continue,
        };

        out.push_str(&input[cursor..whole.start()]);
        cursor = whole.end();

        let pattern = SecretPattern {
            kind: match caps.get(1).map(|m| m.as_str()) {
                Some("secret") => SecretPatternKind::Secret,
                Some("secret-raw") => SecretPatternKind::Raw,
                _ => continue,
            },
            service: caps
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            key: caps
                .get(3)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
        };

        out.push_str(&resolver(pattern).await?);
    }

    out.push_str(&input[cursor..]);
    Ok(out)
}
