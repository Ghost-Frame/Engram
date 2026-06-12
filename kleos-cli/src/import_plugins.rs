//! Plugin importer for the Skills Cloud (v50+).
//!
//! Walks `~/.claude/plugins/installed_plugins.json`, reads each plugin's
//! canonical install dir, and ingests every SKILL.md / agents/*.md /
//! commands/*.md into Kleos as a first-class skill with kind discrimination,
//! content-hash dedup, auto-aliases, and per-plugin bundles.
//!
//! MCP conversion (`.mcp.json` -> workflow skill via /skills/capture) is
//! handled in a separate pass; see `convert_mcp_servers` below.
//!
//! The importer is pure HTTP -- no direct DB access -- so it works just as
//! well from a laptop hitting prod Kleos through defguard as it does in
//! a local-dev loop. Round-trips are minimized but not micro-optimized;
//! ~250 plugin items finishes in seconds against any network we care about.

use crate::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// Plugins that ship pure runtime (LSP, output styles) with no skill-shaped
// content; the importer skips them entirely so we don't pollute the cloud
// with empty / non-content rows.
const SKIP_PLUGINS: &[&str] = &[
    "rust-analyzer-lsp",
    "typescript-lsp",
    "clangd-lsp",
    "explanatory-output-style",
    "learning-output-style",
];

// Plugins that always carry the `pinned` tag as canonical defaults.
// Empty by default; operators can list their own pinned plugins via
// `~/.config/kleos/skill-import.toml`.
const DEFAULT_PINNED: &[&str] = &[];

// Heuristic keyword set for the code-dev classifier. A skill / agent /
// command whose description hits any of these gets `domain:code-dev`. The
// LLM second-pass is a future enhancement; the heuristic is "good enough"
// for the well-known plugin set we're starting with.
const CODE_DEV_KEYWORDS: &[&str] = &[
    "review",
    "refactor",
    "debug",
    "test",
    "verify",
    "plan",
    "spec",
    "architect",
    "explore",
    "security",
    "lint",
    "format",
    "commit",
    " pr ",
    "pull request",
    "simplifier",
    "reviewer",
    "implementation",
    "code",
    "build",
    "compile",
    "deploy",
    "vulnerability",
    "audit",
    "scan",
    "fix",
    "patch",
    "diff",
];

// Initial mapping from plugin name to af-phase tags. Plugins not listed
// here get no af-phase tag (they're not absorbed into Agent-Forge).
//
// First entry per plugin is the dominant phase; agents that span multiple
// phases (e.g. feature-dev's code-explorer = explore, code-architect = spec)
// get phase-tag-per-item via the per-item override in the config file --
// not yet wired, so all items in a plugin get the plugin's first phase.
fn plugin_af_phase(plugin: &str) -> Option<&'static str> {
    match plugin {
        "feature-dev" => Some("af-phase:spec"),
        "superpowers" => Some("af-phase:spec"),
        "pr-review-toolkit" => Some("af-phase:verify"),
        "coderabbit" => Some("af-phase:verify"),
        "code-review" => Some("af-phase:verify"),
        "code-simplifier" => Some("af-phase:verify"),
        "qodo-skills" => Some("af-phase:verify"),
        "aikido" => Some("af-phase:verify"),
        "semgrep" => Some("af-phase:verify"),
        "ai-plugins" => Some("af-phase:verify"),
        "claude-config-validator" => Some("af-phase:verify"),
        "bitwarden-code-review" => Some("af-phase:verify"),
        "bitwarden-security-engineer" => Some("af-phase:challenge"),
        "bitwarden-software-engineer" => Some("af-phase:implement"),
        "bitwarden-tech-lead" => Some("af-phase:spec"),
        "bitwarden-devops-engineer" => Some("af-phase:implement"),
        "bitwarden-product-analyst" => Some("af-phase:spec"),
        "agent-sdk-dev" => Some("af-phase:implement"),
        "mcp-server-dev" => Some("af-phase:implement"),
        "plugin-dev" => Some("af-phase:implement"),
        "commit-commands" => Some("af-phase:retrospect"),
        "claude-retrospective" => Some("af-phase:retrospect"),
        _ => None,
    }
}

// Top-level installed_plugins.json shape. Versions exceed our needs;
// only the install paths matter.
#[derive(Debug, Deserialize)]
struct InstalledPluginsFile {
    plugins: BTreeMap<String, Vec<InstalledPluginEntry>>,
}

/// One version entry for a plugin; carries the filesystem install path.
#[derive(Debug, Deserialize)]
struct InstalledPluginEntry {
    #[serde(rename = "installPath")]
    install_path: String,
    #[allow(dead_code)]
    version: Option<String>,
}

// .claude-plugin/plugin.json shape (only the bits we use).
#[derive(Debug, Default, Deserialize)]
struct PluginManifest {
    #[allow(dead_code)]
    name: Option<String>,
    description: Option<String>,
    version: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
}

// Importer flags surfaced through the CLI. `source_overrides` maps
// plugin-name -> filesystem path to use instead of the cache install path
// (e.g. ralph -> ~/code-improvements/ralph).
pub struct ImportArgs {
    pub dry_run: bool,
    pub plugin_filter: Option<String>,
    pub marketplace_filter: Option<String>,
    pub source_overrides: BTreeMap<String, PathBuf>,
}

// Aggregate stats for the post-run summary.
#[derive(Debug, Default)]
struct Stats {
    plugins_seen: usize,
    plugins_skipped: usize,
    items_seen: usize,
    items_inserted: usize,
    items_updated: usize,
    items_unchanged: usize,
    items_failed: usize,
    bundles_created: usize,
    aliases_created: usize,
}

// Entry point invoked from the CLI dispatch arm.
pub async fn run(client: &Client, args: ImportArgs) {
    let mut stats = Stats::default();
    let installed_path = match resolve_installed_plugins_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Error: {}", e);
            return;
        }
    };

    let installed = match read_installed_plugins(&installed_path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {}", installed_path.display(), e);
            return;
        }
    };

    for (key, entries) in &installed.plugins {
        // Key shape: <plugin>@<marketplace>
        let (plugin_name, marketplace) = match key.split_once('@') {
            Some((p, m)) => (p.to_string(), m.to_string()),
            None => (key.clone(), "unknown".to_string()),
        };

        if let Some(ref p) = args.plugin_filter {
            if &plugin_name != p {
                continue;
            }
        }
        if let Some(ref m) = args.marketplace_filter {
            if &marketplace != m {
                continue;
            }
        }

        if SKIP_PLUGINS.contains(&plugin_name.as_str()) {
            stats.plugins_skipped += 1;
            println!(
                "skip {} ({}): runtime/no-content plugin",
                plugin_name, marketplace
            );
            continue;
        }

        // Resolve the install dir: explicit override beats the cache path.
        let install_dir = args
            .source_overrides
            .get(&plugin_name)
            .cloned()
            .or_else(|| entries.first().map(|e| PathBuf::from(&e.install_path)));
        let install_dir = match install_dir {
            Some(p) if p.exists() => p,
            Some(p) => {
                eprintln!(
                    "skip {}: install path missing: {}",
                    plugin_name,
                    p.display()
                );
                stats.plugins_skipped += 1;
                continue;
            }
            None => {
                stats.plugins_skipped += 1;
                continue;
            }
        };

        stats.plugins_seen += 1;
        let manifest = read_plugin_manifest(&install_dir).unwrap_or_default();

        println!(
            "==> {} ({}) v{} -- {}",
            plugin_name,
            marketplace,
            manifest.version.as_deref().unwrap_or("?"),
            install_dir.display()
        );

        let items = collect_items(&install_dir, &plugin_name, &marketplace, &manifest);
        if items.is_empty() {
            println!("    (no skill content)");
            continue;
        }
        stats.items_seen += items.len();

        // Bundle for this plugin -- created idempotently. Skipped on dry-run.
        let bundle_id = if args.dry_run {
            None
        } else {
            match ensure_plugin_bundle(client, &plugin_name, manifest.description.as_deref()).await
            {
                Ok(id) => {
                    stats.bundles_created += 1;
                    Some(id)
                }
                Err(e) => {
                    eprintln!("    bundle err for {}: {}", plugin_name, e);
                    None
                }
            }
        };

        for item in &items {
            match upsert_item(client, item, bundle_id, &mut stats, args.dry_run).await {
                Ok(_) => {}
                Err(e) => {
                    stats.items_failed += 1;
                    eprintln!("    item err for {}: {}", item.qualified_name, e);
                }
            }
        }
    }

    println!();
    println!(
        "Done. plugins_seen={} plugins_skipped={} items_seen={} \
         inserted={} updated={} unchanged={} failed={} bundles={} aliases={}",
        stats.plugins_seen,
        stats.plugins_skipped,
        stats.items_seen,
        stats.items_inserted,
        stats.items_updated,
        stats.items_unchanged,
        stats.items_failed,
        stats.bundles_created,
        stats.aliases_created,
    );
}

// One canonical plugin item ready for upsert.
struct ImportItem {
    kind: &'static str,
    plugin: String,
    marketplace: String,
    version: Option<String>,
    original_name: String,
    qualified_name: String,
    description: String,
    code: String,
    source_path: String,
    content_hash: String,
    frontmatter: Option<String>,
    tags: Vec<String>,
}

// Walk one plugin install dir and produce ImportItems for skills/agents/
// commands. Hooks and .mcp.json are deferred to a separate pass.
fn collect_items(
    install_dir: &Path,
    plugin: &str,
    marketplace: &str,
    manifest: &PluginManifest,
) -> Vec<ImportItem> {
    let mut out = Vec::new();
    // Skills can be nested arbitrarily deep under skills/. Walk all
    // SKILL.md files. Cap depth implicitly via WalkDir-equivalent.
    walk_kind(
        &install_dir.join("skills"),
        "skill",
        plugin,
        marketplace,
        manifest,
        Some("SKILL.md"),
        &mut out,
    );
    // Agents / commands are flat *.md under their kind dir (mostly).
    walk_kind(
        &install_dir.join("agents"),
        "agent",
        plugin,
        marketplace,
        manifest,
        None,
        &mut out,
    );
    walk_kind(
        &install_dir.join("commands"),
        "command",
        plugin,
        marketplace,
        manifest,
        None,
        &mut out,
    );
    out
}

// Recursive walker. `restrict_filename` lets the skills/ pass match only
// `SKILL.md`; agents/commands pass None to consume every .md.
fn walk_kind(
    root: &Path,
    kind: &'static str,
    plugin: &str,
    marketplace: &str,
    manifest: &PluginManifest,
    restrict_filename: Option<&str>,
    out: &mut Vec<ImportItem>,
) {
    if !root.exists() {
        return;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Do not follow symlinks: a symlinked directory inside an imported
        // plugin tree could escape the marketplace root or form a cycle.
        // DirEntry::file_type does not traverse the link.
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(true) {
            continue;
        }
        if path.is_dir() {
            walk_kind(
                &path,
                kind,
                plugin,
                marketplace,
                manifest,
                restrict_filename,
                out,
            );
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let take = match restrict_filename {
                Some(needle) => name == needle,
                None => name.ends_with(".md") && !name.starts_with("README"),
            };
            if !take {
                continue;
            }
            if let Some(item) = build_item(&path, kind, plugin, marketplace, manifest) {
                out.push(item);
            }
        }
    }
}

// Read a single .md file and prepare an ImportItem.
fn build_item(
    path: &Path,
    kind: &'static str,
    plugin: &str,
    marketplace: &str,
    manifest: &PluginManifest,
) -> Option<ImportItem> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("    read err: {} -- {}", path.display(), e);
            return None;
        }
    };

    let (frontmatter, body) = split_frontmatter(&raw);
    let original_name = frontmatter
        .as_ref()
        .and_then(|fm| extract_yaml_scalar(fm, "name"))
        .or_else(|| {
            // Fall back to the parent dir name for SKILL.md, the file
            // stem for agents/commands.
            if path.file_name().and_then(|n| n.to_str()) == Some("SKILL.md") {
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .map(String::from)
            } else {
                path.file_stem().and_then(|n| n.to_str()).map(String::from)
            }
        })?;

    let description = frontmatter
        .as_ref()
        .and_then(|fm| extract_yaml_scalar(fm, "description"))
        .unwrap_or_default();

    let qualified = format!(
        "{}__{}",
        sanitize_name(plugin),
        sanitize_name(&original_name)
    );

    let mut tags: Vec<String> = vec![
        format!("kind:{}", kind),
        format!("plugin:{}", plugin),
        format!("marketplace:{}", marketplace),
    ];
    if let Some(v) = &manifest.version {
        tags.push(format!("version:{}", v));
    }
    for kw in &manifest.keywords {
        tags.push(kw.to_lowercase());
    }
    if DEFAULT_PINNED.contains(&plugin) {
        tags.push("pinned".to_string());
    }
    if hits_code_dev_heuristic(&description) || hits_code_dev_heuristic(&original_name) {
        tags.push("domain:code-dev".to_string());
    }
    if let Some(phase) = plugin_af_phase(plugin) {
        tags.push(phase.to_string());
    }

    // Content hash spans the body only -- we want re-imports to ignore
    // path / frontmatter cosmetic changes (whitespace, comments) but not
    // body changes. If the upstream needs hash sensitivity to frontmatter
    // tags, switch to hashing `raw`.
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    let hash = hex_digest(&h.finalize());

    Some(ImportItem {
        kind,
        plugin: plugin.to_string(),
        marketplace: marketplace.to_string(),
        version: manifest.version.clone(),
        original_name,
        qualified_name: qualified,
        description,
        code: body.to_string(),
        source_path: path.display().to_string(),
        content_hash: hash,
        frontmatter,
        tags,
    })
}

// Split `---\n...\n---\n` frontmatter from the body. Returns (frontmatter,
// body). When the file has no frontmatter, returns (None, full content).
fn split_frontmatter(raw: &str) -> (Option<String>, &str) {
    if !raw.starts_with("---\n") && !raw.starts_with("---\r\n") {
        return (None, raw);
    }
    let after_open = raw.find('\n').map(|i| i + 1).unwrap_or(raw.len());
    let rest = &raw[after_open..];
    // Find a line that is exactly "---" (optionally followed by whitespace).
    for (line_start, line) in line_offsets(rest) {
        if line.trim_end() == "---" {
            let fm = rest[..line_start].to_string();
            let body_start = line_start + line.len();
            // Skip the trailing newline after the closing ---
            let body = rest[body_start..].trim_start_matches('\n');
            return (Some(fm), body);
        }
    }
    (None, raw)
}

// Iterator-y helper: yield (offset_within_input, line_with_terminator)
// pairs without allocating per-line strings.
fn line_offsets(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        if c == '\n' {
            out.push((start, &s[start..i + 1]));
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push((start, &s[start..]));
    }
    out
}

// Tiny YAML scalar extractor for `key: value` lines. Handles quotes,
// strips leading/trailing whitespace. Does NOT handle nested maps,
// arrays, or multi-line scalars -- the importer only needs name and
// description, both of which are flat in plugin frontmatter.
fn extract_yaml_scalar(fm: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    for line in fm.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
                continue;
            }
            let v = rest.trim();
            // Strip matching outer quotes.
            let v = v
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(v);
            if v.is_empty() {
                return None;
            }
            return Some(v.to_string());
        }
    }
    None
}

// Snake-cased ASCII id used for `Skill.name` / bundle / alias keys.
// Collapses non-alphanum to '_' and lowercases.
fn sanitize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// Returns true if the text contains any keyword from the code-dev heuristic set.
fn hits_code_dev_heuristic(text: &str) -> bool {
    let lc = text.to_lowercase();
    CODE_DEV_KEYWORDS.iter().any(|k| lc.contains(k))
}

// hex_digest is shared with main.rs but inlined here to avoid pulling that
// fn into a module. Trivial.
fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// Resolves the canonical path to `~/.claude/plugins/installed_plugins.json`.
fn resolve_installed_plugins_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME not set".to_string())?;
    Ok(PathBuf::from(format!(
        "{}/.claude/plugins/installed_plugins.json",
        home
    )))
}

/// Reads and deserializes the installed plugins JSON file from disk.
fn read_installed_plugins(path: &Path) -> Result<InstalledPluginsFile, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

/// Reads the `.claude-plugin/plugin.json` manifest from the plugin's install directory.
fn read_plugin_manifest(install_dir: &Path) -> Option<PluginManifest> {
    let path = install_dir.join(".claude-plugin").join("plugin.json");
    let raw = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

// Look up an existing skill row for an item. Returns Some((id, hash))
// when present, None when this is a fresh row.
async fn lookup_existing(client: &Client, item: &ImportItem) -> Option<(i64, String)> {
    // /skills/find with a plugin filter is the cheapest way to find a row
    // by (source_plugin, name). We don't have a dedicated lookup endpoint
    // yet; the find result is filtered server-side via plugin filter and
    // ranked, so the qualified_name typically lands at rank 0.
    let body = json!({
        "query": item.original_name,
        "plugin": item.plugin,
        "limit": 5,
        "include_deprecated": true
    });
    let v = client.post("/skills/find", body).await.ok()?;
    let results = v.get("results").and_then(|r| r.as_array())?;
    for r in results {
        let skill = r.get("skill").cloned().unwrap_or_else(|| r.clone());
        let name = skill.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if name == item.qualified_name {
            let id = skill.get("id").and_then(|x| x.as_i64())?;
            let hash = skill
                .get("content_hash")
                .and_then(|x| x.as_str())
                .map(String::from)
                .unwrap_or_default();
            return Some((id, hash));
        }
    }
    None
}

// Insert / update / skip one item, then attach tags + auto-aliases + bundle.
async fn upsert_item(
    client: &Client,
    item: &ImportItem,
    bundle_id: Option<i64>,
    stats: &mut Stats,
    dry_run: bool,
) -> Result<(), String> {
    if dry_run {
        println!(
            "    [dry] {} ({}) -> {} ({} tags)",
            item.kind,
            item.original_name,
            item.qualified_name,
            item.tags.len()
        );
        return Ok(());
    }

    let metadata = serialize_metadata(item);
    let existing = lookup_existing(client, item).await;
    let id = match existing {
        Some((id, hash)) if hash == item.content_hash => {
            stats.items_unchanged += 1;
            println!("    = #{} {} (unchanged)", id, item.qualified_name);
            id
        }
        Some((id, _)) => {
            // Update body + description + hash.
            let body = json!({
                "code": item.code,
                "description": item.description,
                "kind": item.kind,
                "source_path": item.source_path,
                "content_hash": item.content_hash,
                "metadata": metadata,
            });
            client
                .post(&format!("/skills/{}/update", id), body)
                .await
                .map_err(|e| e.to_string())?;
            stats.items_updated += 1;
            println!("    ~ #{} {}", id, item.qualified_name);
            id
        }
        None => {
            let body = json!({
                "name": item.qualified_name,
                "agent": "claude-code",
                "description": item.description,
                "code": item.code,
                "language": "markdown",
                "kind": item.kind,
                "source_plugin": item.plugin,
                "source_path": item.source_path,
                "content_hash": item.content_hash,
                "metadata": metadata,
                "tags": item.tags,
            });
            let v = client
                .post("/skills", body)
                .await
                .map_err(|e| e.to_string())?;
            let id = v
                .get("id")
                .and_then(|x| x.as_i64())
                .ok_or("no id in /skills response")?;
            stats.items_inserted += 1;
            println!("    + #{} {}", id, item.qualified_name);
            id
        }
    };

    // Auto-aliases: bare name + snake / kebab variants + plugin-qualified
    // shortforms. Confidence per variant matches the lib's `auto_aliases_for`.
    for (alias, conf) in derive_auto_aliases(&item.plugin, &item.original_name) {
        let body = json!({ "alias": alias, "confidence": conf, "source": "auto" });
        if client
            .post(&format!("/skills/{}/aliases", id), body)
            .await
            .is_ok()
        {
            stats.aliases_created += 1;
        }
    }

    if let Some(bid) = bundle_id {
        let body = json!({ "skill_id": id });
        client
            .post(&format!("/bundles/{}/skills", bid), body)
            .await
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

// Mirror of `kleos_lib::skills::aliases::auto_aliases_for` -- duplicated
// here because the CLI doesn't link kleos-lib and we don't want it to.
// Keep these in sync; the lib has the canonical version + tests.
fn derive_auto_aliases(plugin: &str, original_name: &str) -> Vec<(String, f64)> {
    let raw = original_name.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let bare = raw.to_lowercase();
    let snake = bare.replace(['-', ' '], "_");
    let kebab = bare.replace(['_', ' '], "-");
    let mut out: Vec<(String, f64)> = Vec::new();
    let mut push = |s: String, c: f64| {
        if !s.is_empty() && !out.iter().any(|(existing, _)| existing == &s) {
            out.push((s, c));
        }
    };
    push(bare.clone(), 0.9);
    push(snake, 0.85);
    push(kebab, 0.85);
    let p = plugin.to_lowercase();
    push(format!("{p}/{bare}"), 0.95);
    push(format!("{p}:{bare}"), 0.95);
    out
}

// Serialize per-item metadata (frontmatter + plugin context) as a JSON
// string. Matches the shape the materialize code in main.rs reads:
// metadata.frontmatter is the raw YAML block from the source file.
fn serialize_metadata(item: &ImportItem) -> String {
    let mut map = serde_json::Map::new();
    if let Some(ref fm) = item.frontmatter {
        map.insert("frontmatter".into(), Value::String(fm.clone()));
    }
    map.insert("plugin".into(), Value::String(item.plugin.clone()));
    map.insert(
        "marketplace".into(),
        Value::String(item.marketplace.clone()),
    );
    if let Some(ref v) = item.version {
        map.insert("plugin_version".into(), Value::String(v.clone()));
    }
    map.insert(
        "original_name".into(),
        Value::String(item.original_name.clone()),
    );
    serde_json::to_string(&Value::Object(map)).unwrap_or_else(|_| "{}".into())
}

// Idempotently create the per-plugin bundle and return its id.
async fn ensure_plugin_bundle(
    client: &Client,
    plugin: &str,
    description: Option<&str>,
) -> Result<i64, String> {
    let body = json!({
        "name": plugin,
        "description": description.unwrap_or(""),
        "auto_generated": true,
    });
    let v = client
        .post("/bundles", body)
        .await
        .map_err(|e| e.to_string())?;
    v.get("id")
        .and_then(|x| x.as_i64())
        .ok_or_else(|| format!("no id in /bundles response: {}", v))
}

/// Persisted per-user import configuration loaded from `~/.config/kleos/skill-import.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ImportConfig {
    #[serde(default)]
    pub source_overrides: BTreeMap<String, String>,
    #[serde(default)]
    pub pinned_plugins: Vec<String>,
}

// Read ~/.config/kleos/skill-import.toml if it exists. Empty config when
// missing so the importer always runs with sane defaults.
pub fn load_config() -> ImportConfig {
    let home = std::env::var("HOME").unwrap_or_default();
    let path = PathBuf::from(format!("{}/.config/kleos/skill-import.toml", home));
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return ImportConfig::default(),
    };
    toml::from_str(&raw).unwrap_or_default()
}
