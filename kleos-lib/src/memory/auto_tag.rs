pub fn infer_tags(content: &str) -> Vec<String> {
    let lower = content.to_lowercase();
    let mut tags = Vec::new();

    let service_map: &[(&str, &str)] = &[
        ("kleos", "kleos"),
        ("engram", "engram"),
        ("credd", "credd"),
        ("kleos-ingest", "kleos-ingest"),
        ("eidolon", "eidolon"),
    ];
    for &(needle, tag) in service_map {
        if lower.contains(needle) {
            tags.push(tag.to_string());
        }
    }

    let verb_map: &[(&str, &str)] = &[
        ("deployed", "deploy"),
        ("shipped", "deploy"),
        ("pushed to", "deploy"),
        ("fixed", "fix"),
        ("bug", "bug"),
        ("decided", "decision"),
        ("setup", "setup"),
        ("configured", "setup"),
    ];
    for &(needle, tag) in verb_map {
        if lower.contains(needle) {
            tags.push(tag.to_string());
        }
    }

    tags.sort();
    tags.dedup();
    tags
}

pub fn infer_category(content: &str) -> Option<&'static str> {
    let lower = content.to_lowercase();

    if lower.contains("deployed") || lower.contains("shipped") || lower.contains("pushed to") {
        return Some("state");
    }
    if lower.contains("decided") || lower.contains("chose") || lower.contains("rejected") {
        return Some("decision");
    }
    if lower.contains("fixed") || lower.contains("bug") || lower.contains("broken") || lower.contains("error") {
        return Some("issue");
    }
    if lower.contains("how to") || lower.contains("step 1") || lower.contains("step 2") {
        return Some("reference");
    }
    None
}
