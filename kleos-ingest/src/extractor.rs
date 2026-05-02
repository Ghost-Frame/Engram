use std::collections::HashMap;
use std::time::Instant;

pub struct MemoryCandidate {
    pub content: String,
    pub category: String,
    pub tags: Vec<String>,
    pub importance: i32,
    pub source: String,
    pub session_id: String,
    pub project: String,
}

pub struct Extractor {
    recent_hashes: HashMap<String, Instant>,
}

impl Extractor {
    pub fn new() -> Self {
        Self {
            recent_hashes: HashMap::new(),
        }
    }

    pub fn extract(&mut self, turn_text: &str, session_id: &str, project: &str) -> Option<MemoryCandidate> {
        let trimmed = turn_text.trim();

        // Skip short turns (quick acks)
        if trimmed.len() < 50 {
            return None;
        }

        // In-session dedup: skip if same 200-char prefix seen in last 10 min
        let prefix: String = trimmed.chars().take(200).collect();
        let now = Instant::now();
        self.recent_hashes.retain(|_, t| now.duration_since(*t).as_secs() < 600);
        if self.recent_hashes.contains_key(&prefix) {
            return None;
        }

        let lower = trimmed.to_lowercase();
        let mut tags = Vec::new();
        let mut category = "general";
        let mut importance = 5;

        // Classification rules
        if contains_any(&lower, &["deployed", "shipped", "pushed to"]) || has_commit_hash(trimmed) {
            category = "state";
            importance = 6;
            tags.push("deploy".to_string());
            infer_service_tags(&lower, &mut tags);
        } else if contains_any(&lower, &["decided", "chose", "rejected", "picked", "went with"]) {
            category = "decision";
            importance = 8;
            tags.push("decision".to_string());
        } else if contains_any(&lower, &["i want", "i prefer", "don't do", "always do", "never do", "from now on"]) {
            category = "decision";
            importance = 7;
            tags.push("preference".to_string());
        } else if contains_any(&lower, &["fixed", "bug", "broken", "error", "stack trace", "panic"]) {
            category = "issue";
            importance = 7;
            infer_service_tags(&lower, &mut tags);
        } else if contains_any(&lower, &["how to", "step 1", "step 2", "steps:", "procedure"]) {
            category = "reference";
            importance = 5;
        } else if has_infra_info(&lower) {
            category = "infrastructure";
            importance = 6;
            infer_host_tags(&lower, &mut tags);
        } else {
            // No strong signal -- skip
            return None;
        }

        self.recent_hashes.insert(prefix, now);

        Some(MemoryCandidate {
            content: if trimmed.len() > 2000 {
                trimmed[..2000].to_string()
            } else {
                trimmed.to_string()
            },
            category: category.to_string(),
            tags,
            importance,
            source: "kleos-ingest".to_string(),
            session_id: session_id.to_string(),
            project: project.to_string(),
        })
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn has_commit_hash(text: &str) -> bool {
    text.split_whitespace().any(|word| {
        word.len() >= 7 && word.len() <= 40 && word.chars().all(|c| c.is_ascii_hexdigit())
    })
}

fn has_infra_info(lower: &str) -> bool {
    contains_any(lower, &["172.", "192.168.", "10.", "port ", ":4200", ":8080", ":443", "ssh ", "systemctl"])
}

fn infer_service_tags(lower: &str, tags: &mut Vec<String>) {
    let services = [
        "kleos", "engram", "credd",
        "eidolon", "kleos-ingest",
    ];
    for svc in services {
        if lower.contains(svc) {
            tags.push(svc.to_string());
        }
    }
}

fn infer_host_tags(_lower: &str, _tags: &mut Vec<String>) {
}
