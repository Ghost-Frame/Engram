use std::env;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::LazyLock;

mod observe;

const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "c", "cpp", "h", "hpp",
    "java", "rb", "swift", "kt", "scala", "zig", "hs", "ml", "ex", "exs",
    "lua", "pl", "pm", "sh", "bash", "zsh", "fish",
];

const RAW_READ_THRESHOLD: u64 = 8192;

/// Resolve the operator-configured allowlist of write roots. KLEOS_FS_ALLOWED_ROOTS
/// is a colon-separated list of absolute directories; if unset the only
/// allowed root is the current working directory. Roots that fail to
/// canonicalize (do not exist, permission denied) are dropped silently so a
/// stale entry cannot lock the binary out.
fn allowed_roots() -> Vec<PathBuf> {
    let raw = match env::var("KLEOS_FS_ALLOWED_ROOTS") {
        Ok(v) if !v.is_empty() => v,
        _ => match env::current_dir() {
            Ok(d) => d.to_string_lossy().to_string(),
            Err(_) => return Vec::new(),
        },
    };
    raw.split(':')
        .filter(|s| !s.is_empty())
        .filter_map(|s| PathBuf::from(s).canonicalize().ok())
        .collect()
}

/// Resolve `path` to a canonical PathBuf and verify it lies under one of the
/// configured roots. Returns the canonical path on success.
///
/// For NEW files the parent directory must already exist and lie within an
/// allowed root; the leaf is appended after the parent canonicalizes. This
/// blocks `kw ../etc/passwd`, symlink traversal, and `kw foo/../../etc/sudoers`.
fn canonicalize_within_roots(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    if roots.is_empty() {
        return None;
    }
    let resolved = if path.exists() {
        path.canonicalize().ok()?
    } else {
        let parent = path.parent()?;
        let parent_canon = parent.canonicalize().ok()?;
        let leaf = path.file_name()?;
        parent_canon.join(leaf)
    };
    if roots.iter().any(|r| resolved.starts_with(r)) {
        Some(resolved)
    } else {
        None
    }
}

fn main() -> ExitCode {
    let binary_name = env::args()
        .next()
        .and_then(|a| Path::new(&a).file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "kr".to_string());

    let args: Vec<String> = env::args().skip(1).collect();

    match binary_name.as_str() {
        "kr" => cmd_kr(&args),
        "kw" => cmd_kw(&args),
        "ke" => cmd_ke(&args),
        _ => {
            eprintln!("Unknown binary name: {}. Expected kr, kw, or ke.", binary_name);
            ExitCode::from(2)
        }
    }
}

fn cmd_kr(args: &[String]) -> ExitCode {
    let (path, symbol) = match parse_kr_args(args) {
        Some(v) => v,
        None => {
            eprintln!("Usage: kr <path> [--symbol NAME]");
            return ExitCode::from(2);
        }
    };

    let path = match resolve_path(&path) {
        Some(p) => p,
        None => {
            eprintln!("File not found: {}", path);
            return ExitCode::from(1);
        }
    };

    if !path.is_file() {
        eprintln!("Not a file: {}", path.display());
        return ExitCode::from(1);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let is_code = CODE_EXTENSIONS.contains(&ext);
    let file_size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

    // For code files above the threshold, delegate to agent-forge
    if is_code && file_size > RAW_READ_THRESHOLD {
        if let Some(output) = agent_forge_read(&path, symbol.as_deref()) {
            print!("{}", output);
            return ExitCode::SUCCESS;
        }
        // Fall through to raw read on agent-forge failure
    }

    // Raw read
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            print!("{}", content);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error reading {}: {}", path.display(), e);
            ExitCode::from(1)
        }
    }
}

fn cmd_kw(args: &[String]) -> ExitCode {
    let mut path: Option<String> = None;
    let mut allow_mkdir = false;
    for arg in args {
        if arg == "--mkdir" {
            allow_mkdir = true;
        } else if path.is_none() {
            path = Some(arg.clone());
        }
    }

    let raw_path = match path {
        Some(p) => p,
        None => {
            eprintln!("Usage: kw [--mkdir] <path> < content");
            return ExitCode::from(2);
        }
    };

    let path = PathBuf::from(&raw_path);

    let roots = allowed_roots();
    if roots.is_empty() {
        eprintln!(
            "kw: KLEOS_FS_ALLOWED_ROOTS unset and CWD unresolvable; refusing to write"
        );
        return ExitCode::from(2);
    }

    let mut content = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut content) {
        eprintln!("Error reading stdin: {}", e);
        return ExitCode::from(1);
    }

    // If the parent directory does not exist, only create it when --mkdir is
    // given AND the parent itself is inside an allowed root.
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if !allow_mkdir {
                eprintln!(
                    "kw: parent directory {} does not exist; pass --mkdir to create",
                    parent.display()
                );
                return ExitCode::from(2);
            }
            // Walk up to the first existing ancestor and verify IT is inside
            // a root before we mkdir downward.
            let mut ancestor = parent.to_path_buf();
            let existing_ancestor = loop {
                if ancestor.exists() {
                    break ancestor;
                }
                match ancestor.parent() {
                    Some(p) => ancestor = p.to_path_buf(),
                    None => {
                        eprintln!("kw: no existing ancestor for {}", parent.display());
                        return ExitCode::from(2);
                    }
                }
            };
            let canon = match existing_ancestor.canonicalize() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!(
                        "kw: cannot canonicalize ancestor {}: {}",
                        existing_ancestor.display(),
                        e
                    );
                    return ExitCode::from(2);
                }
            };
            if !roots.iter().any(|r| canon.starts_with(r)) {
                eprintln!(
                    "kw: {} resolves outside KLEOS_FS_ALLOWED_ROOTS",
                    parent.display()
                );
                return ExitCode::from(2);
            }
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("Error creating directory {}: {}", parent.display(), e);
                return ExitCode::from(1);
            }
        }
    }

    let target = match canonicalize_within_roots(&path, &roots) {
        Some(p) => p,
        None => {
            eprintln!(
                "kw: {} is outside KLEOS_FS_ALLOWED_ROOTS (set the env var or run from inside an allowed root)",
                path.display()
            );
            return ExitCode::from(2);
        }
    };

    match std::fs::write(&target, &content) {
        Ok(()) => {
            eprintln!("Wrote {} bytes to {}", content.len(), target.display());
            observe::fire_and_forget("kw", &target.to_string_lossy(), None);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error writing {}: {}", target.display(), e);
            ExitCode::from(1)
        }
    }
}

fn cmd_ke(args: &[String]) -> ExitCode {
    let raw_path = match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("Usage: ke <path>");
            return ExitCode::from(2);
        }
    };

    // Same allowlist applies to `ke` so a traversal cannot bypass via the
    // edit path. ke does not write itself; the agent does. But the spec-task
    // ledger key embeds the path, so we still pin the path under a root.
    let roots = allowed_roots();
    if roots.is_empty() {
        eprintln!(
            "ke: KLEOS_FS_ALLOWED_ROOTS unset and CWD unresolvable; refusing"
        );
        return ExitCode::from(2);
    }
    let path_buf = PathBuf::from(&raw_path);
    let path = match canonicalize_within_roots(&path_buf, &roots) {
        Some(p) => p.to_string_lossy().into_owned(),
        None => {
            eprintln!(
                "ke: {} is outside KLEOS_FS_ALLOWED_ROOTS",
                raw_path
            );
            return ExitCode::from(2);
        }
    };

    let session_id = env::var("KLEOS_SESSION_ID")
        .or_else(|_| env::var("CLAUDE_SESSION_ID"))
        .unwrap_or_default();

    let ledger_key = format!("{}:{}", session_id, path);

    match check_scratchpad_ledger(&ledger_key) {
        LedgerResult::Found => {
            eprintln!("Spec-task ledger entry found for {}", path);
            observe::fire_and_forget("ke", &path, None);
            ExitCode::SUCCESS
        }
        LedgerResult::NotFound => {
            eprintln!("BLOCKED: No spec-task in scratchpad ledger for this session.");
            eprintln!("Run: agent-forge --input <spec.json> --output <out.json> spec-task");
            eprintln!("Then retry: ke {}", path);
            ExitCode::from(2)
        }
        LedgerResult::ServerUnavailable => {
            // M-009: fail-closed by default; operator opt-in via env var.
            if env::var("KLEOS_FS_ALLOW_OFFLINE_EDIT").as_deref() == Ok("1") {
                eprintln!("Warning: scratchpad unreachable, allowing edit (KLEOS_FS_ALLOW_OFFLINE_EDIT=1)");
                observe::fire_and_forget("ke", &path, None);
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "BLOCKED: scratchpad ledger unreachable and KLEOS_FS_ALLOW_OFFLINE_EDIT is not set."
                );
                eprintln!("Set KLEOS_FS_ALLOW_OFFLINE_EDIT=1 to allow offline edits.");
                ExitCode::from(2)
            }
        }
    }
}

fn parse_kr_args(args: &[String]) -> Option<(String, Option<String>)> {
    if args.is_empty() {
        return None;
    }

    let mut path = None;
    let mut symbol = None;
    let mut i = 0;

    while i < args.len() {
        if args[i] == "--symbol" {
            i += 1;
            if i < args.len() {
                symbol = Some(args[i].clone());
            }
        } else if path.is_none() {
            path = Some(args[i].clone());
        }
        i += 1;
    }

    path.map(|p| (p, symbol))
}

fn resolve_path(path: &str) -> Option<PathBuf> {
    let p = if path.starts_with("~/") {
        let home = env::var("HOME").ok()?;
        PathBuf::from(home).join(path.strip_prefix("~/")?)
    } else {
        PathBuf::from(path)
    };

    if !p.exists() {
        return None;
    }

    // Canonicalize so `..` segments and symlinks collapse to their real
    // location. The kr binary still allows reading anywhere the process can
    // read, but the resolved path is now stable for downstream tools (e.g.
    // the agent-forge integration that derives input paths from this).
    p.canonicalize().ok()
}

fn agent_forge_read(path: &Path, symbol: Option<&str>) -> Option<String> {
    let forge_bin = find_agent_forge()?;

    // CWE-377: use unguessable temp file names so concurrent invocations
    // cannot collide and a local user cannot pre-create the path as a
    // symlink. NamedTempFile cleans up on drop.
    let input_tmp = tempfile::Builder::new()
        .prefix("kleos-fs-input-")
        .suffix(".json")
        .tempfile()
        .ok()?;
    let output_tmp = tempfile::Builder::new()
        .prefix("kleos-fs-output-")
        .suffix(".json")
        .tempfile()
        .ok()?;
    let input_path = input_tmp.path().to_path_buf();
    let output_path = output_tmp.path().to_path_buf();

    let input_json = if let Some(sym) = symbol {
        serde_json::json!({
            "query": sym,
            "path": path.parent().unwrap_or(Path::new(".")).to_string_lossy(),
            "limit": 10,
        })
    } else {
        serde_json::json!({
            "path": path.parent().unwrap_or(Path::new(".")).to_string_lossy(),
            "focus": [path.file_name().unwrap_or_default().to_string_lossy()],
            "max_tokens": 4000,
        })
    };

    std::fs::write(&input_path, serde_json::to_string(&input_json).ok()?).ok()?;

    let subcommand = if symbol.is_some() {
        "search-code"
    } else {
        "repo-map"
    };

    let status = Command::new(&forge_bin)
        .arg("--input")
        .arg(&input_path)
        .arg("--output")
        .arg(&output_path)
        .arg(subcommand)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;

    if !status.success() {
        return None;
    }

    let output_raw = std::fs::read_to_string(&output_path).ok()?;
    let output: serde_json::Value = serde_json::from_str(&output_raw).ok()?;

    if !output.get("success")?.as_bool()? {
        return None;
    }

    if let Some(data) = output.get("data") {
        Some(serde_json::to_string_pretty(data).unwrap_or_default())
    } else {
        output.get("message").and_then(|m| m.as_str()).map(String::from)
    }
}

fn find_agent_forge() -> Option<PathBuf> {
    if let Ok(path) = env::var("AGENT_FORGE_BIN") {
        let p = PathBuf::from(&path);
        if p.exists() {
            return Some(p);
        }
    }

    let home = env::var("HOME").ok()?;
    let local_bin = PathBuf::from(&home).join(".local/bin/agent-forge");
    if local_bin.exists() {
        return Some(local_bin);
    }

    // M-010: PATH fallback only when operator explicitly trusts PATH.
    // Prevents a malicious binary named agent-forge earlier on PATH from
    // being executed with full agent privileges.
    if env::var("KLEOS_FS_TRUST_PATH").as_deref() == Ok("1") {
        which_in_path("agent-forge")
    } else {
        eprintln!(
            "ke: agent-forge not found at AGENT_FORGE_BIN or ~/.local/bin/agent-forge; \
             set KLEOS_FS_TRUST_PATH=1 to allow PATH lookup"
        );
        None
    }
}

fn which_in_path(name: &str) -> Option<PathBuf> {
    env::var("PATH").ok().and_then(|paths| {
        paths.split(':').find_map(|dir| {
            let candidate = PathBuf::from(dir).join(name);
            if candidate.exists() { Some(candidate) } else { None }
        })
    })
}

enum LedgerResult {
    Found,
    NotFound,
    ServerUnavailable,
}

// M-012: process-lifetime reqwest blocking client replaces the curl shell-out.
// LazyLock so initialisation happens once per process on first use.
static HTTP_CLIENT: LazyLock<reqwest::blocking::Client> = LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap_or_default()
});

fn check_scratchpad_ledger(key: &str) -> LedgerResult {
    let server_url = env::var("KLEOS_SERVER_URL")
        .or_else(|_| env::var("ENGRAM_EIDOLON_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:4200".to_string());

    let api_key = resolve_api_key();

    let url = format!(
        "{}/scratchpad/get?namespace=spec-task&key={}",
        server_url,
        urlencoded(key)
    );

    let mut req = HTTP_CLIENT.get(&url);
    if let Some(ref k) = api_key {
        req = req.header("Authorization", format!("Bearer {}", k));
    }

    let resp = match req.send() {
        Ok(r) => r,
        // Connection errors (refused, timeout, DNS) mean server is unreachable.
        Err(e) if e.is_connect() || e.is_timeout() => {
            return LedgerResult::ServerUnavailable;
        }
        Err(_) => return LedgerResult::ServerUnavailable,
    };

    if !resp.status().is_success() {
        // 404 means ledger entry missing; other errors treated as unavailable.
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return LedgerResult::NotFound;
        }
        return LedgerResult::ServerUnavailable;
    }

    let body = match resp.text() {
        Ok(t) => t,
        Err(_) => return LedgerResult::ServerUnavailable,
    };

    if body.trim().is_empty() || body.contains("\"value\":null") || body.contains("not found") {
        return LedgerResult::NotFound;
    }

    LedgerResult::Found
}

fn resolve_api_key() -> Option<String> {
    if let Ok(key) = env::var("KLEOS_API_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    if let Ok(key) = env::var("EIDOLON_KEY") {
        if !key.is_empty() {
            return Some(key);
        }
    }
    None
}

fn urlencoded(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleanup_temp(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn canonicalize_within_roots_accepts_target_inside_root() {
        let dir = std::env::temp_dir().join(format!("kleos-fs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk root");
        let target = dir.join("ok.txt");
        std::fs::write(&target, "hi").expect("seed");
        let canon_root = dir.canonicalize().expect("canon root");
        let result = canonicalize_within_roots(&target, std::slice::from_ref(&canon_root));
        assert!(result.is_some(), "target inside root must resolve");
        assert!(result.unwrap().starts_with(&canon_root));
        cleanup_temp(&dir);
    }

    #[test]
    fn canonicalize_within_roots_rejects_target_outside_root() {
        let dir = std::env::temp_dir().join(format!("kleos-fs-test-out-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk root");
        let canon_root = dir.canonicalize().expect("canon root");
        // /etc/hostname exists on every Linux box and is outside our temp root.
        let outside = PathBuf::from("/etc/hostname");
        let result = canonicalize_within_roots(&outside, &[canon_root]);
        assert!(result.is_none(), "target outside root must be rejected");
        cleanup_temp(&dir);
    }

    #[test]
    fn canonicalize_within_roots_rejects_traversal() {
        let dir = std::env::temp_dir().join(format!("kleos-fs-test-trav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk root");
        let canon_root = dir.canonicalize().expect("canon root");
        // Construct a path that LOOKS inside but resolves elsewhere via ..
        let traversal = dir.join("../../../etc/hostname");
        let result = canonicalize_within_roots(&traversal, &[canon_root]);
        assert!(
            result.is_none(),
            "traversal path resolving outside root must be rejected"
        );
        cleanup_temp(&dir);
    }

    #[test]
    fn canonicalize_within_roots_handles_new_file_under_root() {
        let dir = std::env::temp_dir().join(format!("kleos-fs-test-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mk root");
        let canon_root = dir.canonicalize().expect("canon root");
        let new_file = dir.join("not-yet-created.txt");
        let result = canonicalize_within_roots(&new_file, std::slice::from_ref(&canon_root));
        assert!(result.is_some(), "new file in existing root must resolve");
        assert!(result.unwrap().starts_with(&canon_root));
        cleanup_temp(&dir);
    }

    #[test]
    fn canonicalize_within_roots_empty_roots_rejects() {
        let p = PathBuf::from("/tmp");
        assert!(canonicalize_within_roots(&p, &[]).is_none());
    }
}
