use super::types::{DiscoveredSkill, SkillMeta};
use regex::Regex;
use std::path::{Path, PathBuf};

/// Parse YAML frontmatter from SKILL.md content.
pub fn parse_skill_md(raw: &str) -> SkillMeta {
    let mut meta = SkillMeta::default();
    if !raw.starts_with("---") {
        let re = Regex::new(r"(?m)^#\s+(.+)$").unwrap();
        if let Some(caps) = re.captures(raw) {
            meta.name = caps[1].trim().to_string();
        }
        return meta;
    }
    let end_idx = match raw[3..].find("\n---") {
        Some(i) => i + 3,
        None => return meta,
    };
    let frontmatter = &raw[4..end_idx];
    let kv_re = Regex::new(r"(?m)^(\w+)\s*:\s*(.+)$").unwrap();
    for caps in kv_re.captures_iter(frontmatter) {
        let key = &caps[1];
        let raw_val = caps[2].trim();
        let val = raw_val.trim_matches('"').trim().to_string();
        match key {
            "name" => meta.name = val,
            "description" => meta.description = val,
            "category" => meta.category = Some(val),
            "tags" => {
                let inner = val.trim_start_matches('[').trim_end_matches(']');
                meta.tags = Some(
                    inner
                        .split(',')
                        .map(|t| t.trim().trim_matches('"').to_string())
                        .filter(|t| !t.is_empty())
                        .collect(),
                );
            }
            _ => {}
        }
    }
    if meta.name.is_empty() {
        let body = &raw[end_idx + 4..];
        let re = Regex::new(r"(?m)^#\s+(.+)$").unwrap();
        if let Some(caps) = re.captures(body) {
            meta.name = caps[1].trim().to_string();
        }
    }
    meta
}

pub fn read_skill_id(skill_dir: &Path) -> Option<String> {
    std::fs::read_to_string(skill_dir.join(".skill_id"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn write_skill_id(skill_dir: &Path, id: &str) -> std::io::Result<()> {
    std::fs::write(skill_dir.join(".skill_id"), format!("{}\n", id))
}

fn walk_skill_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return found,
    };
    for entry in entries.flatten() {
        let name_str = entry.file_name().to_string_lossy().to_string();
        if name_str.starts_with('.') || name_str == "node_modules" {
            continue;
        }
        let full = entry.path();
        if !full.is_dir() {
            continue;
        }
        if full.join("SKILL.md").exists() {
            found.push(full);
        } else {
            found.extend(walk_skill_dirs(&full));
        }
    }
    found
}

pub fn discover_skills(dirs: &[&str]) -> Vec<DiscoveredSkill> {
    let mut found = Vec::new();
    for dir in dirs {
        let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));
        for skill_dir in walk_skill_dirs(&abs) {
            let content = match std::fs::read_to_string(skill_dir.join("SKILL.md")) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let meta = parse_skill_md(&content);
            if meta.name.is_empty() {
                continue;
            }
            let skill_id = read_skill_id(&skill_dir).unwrap_or_else(|| {
                let safe: String = meta
                    .name
                    .to_lowercase()
                    .chars()
                    .map(|c| if c.is_alphanumeric() { c } else { '-' })
                    .collect();
                let safe = &safe[..safe.len().min(40)];
                let hex = &uuid::Uuid::new_v4().to_string().replace('-', "")[..8];
                let id = format!("{}__imp_{}", safe, hex);
                let _ = write_skill_id(&skill_dir, &id);
                id
            });
            found.push(DiscoveredSkill {
                skill_id,
                path: skill_dir,
                content,
                meta,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_parse_frontmatter() {
        let raw = "---\nname: Test\ndescription: A test\ntags: [a, b]\n---\n# Body";
        let m = parse_skill_md(raw);
        assert_eq!(m.name, "Test");
        assert_eq!(m.tags.unwrap().len(), 2);
    }
    #[test]
    fn test_parse_no_frontmatter() {
        let m = parse_skill_md("# My Skill\nBody");
        assert_eq!(m.name, "My Skill");
    }
    #[test]
    fn test_parse_empty() {
        assert!(parse_skill_md("").name.is_empty());
    }
}
