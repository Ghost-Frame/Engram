use crate::{EngError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Sanitize a path component to prevent directory traversal.
pub fn sanitize_path(p: &str) -> Result<String> {
    let normalized = p.replace('\\', "/");
    if normalized.contains("..") || normalized.starts_with('/') {
        return Err(EngError::InvalidInput(format!("path traversal not allowed: {}", p)));
    }
    Ok(normalized)
}

/// Collect all files in a skill directory into a map.
pub fn collect_skill_snapshot(dir: &Path) -> Result<HashMap<String, String>> {
    let mut snapshot = HashMap::new();
    if !dir.exists() { return Ok(snapshot); }
    collect_recursive(dir, dir, &mut snapshot)?;
    Ok(snapshot)
}
fn collect_recursive(base: &Path, current: &Path, snapshot: &mut HashMap<String, String>) -> Result<()> {
    let entries = std::fs::read_dir(current).map_err(|e| EngError::Internal(format!("read dir: {}", e)))?;
    for entry in entries {
        let entry = entry.map_err(|e| EngError::Internal(format!("dir entry: {}", e)))?;
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) { continue; }
            collect_recursive(base, &path, snapshot)?;
        } else {
            let rel = path.strip_prefix(base)
                .map_err(|e| EngError::Internal(format!("strip prefix: {}", e)))?
                .to_string_lossy()
                .replace('\\', "/");
            let content = std::fs::read_to_string(&path).map_err(|e| EngError::Internal(format!("read file: {}", e)))?;
            snapshot.insert(rel, content);
        }
    }
    Ok(())
}

/// Compute a simple unified diff between two snapshots.
pub fn compute_unified_diff(before: &HashMap<String, String>, after: &HashMap<String, String>) -> String {
    let mut diff = String::new();
    let mut all_keys: Vec<&String> = before.keys().chain(after.keys()).collect();
    all_keys.sort();
    all_keys.dedup();
    for key in all_keys {
        let old = before.get(key).map(|s| s.as_str()).unwrap_or("");
        let new_val = after.get(key).map(|s| s.as_str()).unwrap_or("");
        if old == new_val { continue; }
        diff.push_str(&format!("--- a/{}\n", key));
        diff.push_str(&format!("+++ b/{}\n", key));
        let old_lines: Vec<&str> = old.lines().collect();
        let new_lines: Vec<&str> = new_val.lines().collect();
        for line in &old_lines { if !new_lines.contains(line) { diff.push_str(&format!("-{}\n", line)); } }
        for line in &new_lines { if !old_lines.contains(line) { diff.push_str(&format!("+{}\n", line)); } }
    }
    diff
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetectedPatchType { Full, SearchReplace, MultiFile }

/// Detect what kind of patch content we have.
pub fn detect_patch_type(content: &str) -> DetectedPatchType {
    if content.contains("<<<<<<< SEARCH") { DetectedPatchType::SearchReplace }
    else if content.contains("*** File:") { DetectedPatchType::MultiFile }
    else { DetectedPatchType::Full }
}

/// Apply SEARCH/REPLACE blocks to existing content.
pub fn apply_search_replace(original: &str, patch: &str) -> Result<String> {
    let mut result = original.to_string();
    let blocks = parse_search_replace_blocks(patch)?;
    for (search, replace) in blocks {
        if !result.contains(&search) {
            return Err(EngError::InvalidInput(format!("SEARCH block not found: {}", &search[..search.len().min(80)])));
        }
        result = result.replace(&search, &replace);
    }
    Ok(result)
}

fn parse_search_replace_blocks(patch: &str) -> Result<Vec<(String, String)>> {
    let mut blocks = Vec::new();
    let lines: Vec<&str> = patch.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "<<<<<<< SEARCH" {
            let mut search_lines = Vec::new();
            i += 1;
            while i < lines.len() && lines[i].trim() != "=======" { search_lines.push(lines[i]); i += 1; }
            i += 1;
            let mut replace_lines = Vec::new();
            while i < lines.len() && lines[i].trim() != ">>>>>>> REPLACE" { replace_lines.push(lines[i]); i += 1; }
            blocks.push((search_lines.join("\n"), replace_lines.join("\n")));
        }
        i += 1;
    }
    Ok(blocks)
}
/// Parse multi-file envelope format.
pub fn parse_multi_file(content: &str) -> HashMap<String, String> {
    let mut files = HashMap::new();
    let mut current_file: Option<String> = None;
    let mut current_content = Vec::new();
    for line in content.lines() {
        if line.starts_with("*** File:") {
            if let Some(ref name) = current_file { files.insert(name.clone(), current_content.join("\n")); }
            current_file = Some(line.trim_start_matches("*** File:").trim().to_string());
            current_content.clear();
        } else { current_content.push(line.to_string()); }
    }
    if let Some(name) = current_file { files.insert(name, current_content.join("\n")); }
    files
}

/// Create a new skill directory with content files.
pub fn create_skill_on_disk(base_dir: &Path, skill_name: &str, content: &str) -> Result<PathBuf> {
    let dir = base_dir.join(skill_name);
    std::fs::create_dir_all(&dir).map_err(|e| EngError::Internal(format!("create dir: {}", e)))?;
    match detect_patch_type(content) {
        DetectedPatchType::MultiFile => {
            for (name, file_content) in parse_multi_file(content) {
                let safe_name = sanitize_path(&name)?;
                let file_path = dir.join(&safe_name);
                if let Some(parent) = file_path.parent() { std::fs::create_dir_all(parent).map_err(|e| EngError::Internal(format!("mkdir: {}", e)))?; }
                std::fs::write(&file_path, &file_content).map_err(|e| EngError::Internal(format!("write: {}", e)))?;
            }
        }
        _ => {
            std::fs::write(dir.join("SKILL.md"), content).map_err(|e| EngError::Internal(format!("write: {}", e)))?;
        }
    }
    Ok(dir)
}

/// Fix existing skill files in-place using a patch.
pub fn fix_skill_files(skill_dir: &Path, patch_content: &str) -> Result<String> {
    match detect_patch_type(patch_content) {
        DetectedPatchType::SearchReplace => {
            let skill_file = skill_dir.join("SKILL.md");
            let original = std::fs::read_to_string(&skill_file).map_err(|e| EngError::Internal(format!("read: {}", e)))?;
            let patched = apply_search_replace(&original, patch_content)?;
            std::fs::write(&skill_file, &patched).map_err(|e| EngError::Internal(format!("write: {}", e)))?;
            Ok(patched)
        }
        DetectedPatchType::MultiFile => {
            let mut combined = String::new();
            for (name, content) in parse_multi_file(patch_content) {
                let safe_name = sanitize_path(&name)?;
                let file_path = skill_dir.join(&safe_name);
                if let Some(parent) = file_path.parent() { std::fs::create_dir_all(parent).map_err(|e| EngError::Internal(format!("mkdir: {}", e)))?; }
                std::fs::write(&file_path, &content).map_err(|e| EngError::Internal(format!("write: {}", e)))?;
                combined.push_str(&format!("--- {} ---\n{}\n", name, content));
            }
            Ok(combined)
        }
        DetectedPatchType::Full => {
            std::fs::write(skill_dir.join("SKILL.md"), patch_content).map_err(|e| EngError::Internal(format!("write: {}", e)))?;
            Ok(patch_content.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_sanitize_ok() { assert_eq!(sanitize_path("foo/bar.md").unwrap(), "foo/bar.md"); }
    #[test] fn test_sanitize_traversal() { assert!(sanitize_path("../etc/passwd").is_err()); }
    #[test] fn test_sanitize_absolute() { assert!(sanitize_path("/etc/passwd").is_err()); }
    #[test] fn test_detect_full() { assert_eq!(detect_patch_type("content"), DetectedPatchType::Full); }
    #[test] fn test_detect_sr() { assert_eq!(detect_patch_type("<<<<<<< SEARCH"), DetectedPatchType::SearchReplace); }
    #[test] fn test_detect_multi() { assert_eq!(detect_patch_type("*** File: a.md"), DetectedPatchType::MultiFile); }
    #[test] fn test_apply_sr() {
        let result = apply_search_replace("hello world", "<<<<<<< SEARCH\nhello world\n=======\nhello rust\n>>>>>>> REPLACE").unwrap();
        assert_eq!(result, "hello rust");
    }
    #[test] fn test_parse_multi() {
        let files = parse_multi_file("*** File: a.md\ncontent a\n*** File: b.md\ncontent b");
        assert_eq!(files.len(), 2);
    }
    #[test] fn test_diff_no_change() {
        let mut a = HashMap::new(); a.insert("f.md".into(), "hello".into());
        assert!(compute_unified_diff(&a, &a).is_empty());
    }
}