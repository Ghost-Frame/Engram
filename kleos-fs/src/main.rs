use std::env;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

mod observe;

const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "c", "cpp", "h", "hpp",
    "java", "rb", "swift", "kt", "scala", "zig", "hs", "ml", "ex", "exs",
    "lua", "pl", "pm", "sh", "bash", "zsh", "fish",
];

const RAW_READ_THRESHOLD: u64 = 8192;
// Capped raw fallback: when agent-forge fails on a large code file, emit
// only head + tail to avoid silently undoing the token-budget promise.
const RAW_FALLBACK_HEAD: usize = 4096;
const RAW_FALLBACK_TAIL: usize = 4096;

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

    // For code files above the threshold, delegate to agent-forge.
    if is_code && file_size > RAW_READ_THRESHOLD {
        match agent_forge_read(&path, symbol.as_deref()) {
            Ok(output) => {
                print!("{}", output);
                return ExitCode::SUCCESS;
            }
            Err(err) => {
                eprintln!("kleos-fs: agent-forge fallback ({}); reading raw", err);
                if env::var("KLEOS_FS_NO_FALLBACK")
                    .map(|v| !v.is_empty() && v != "0")
                    .unwrap_or(false)
                {
                    return ExitCode::from(1);
                }
                return raw_fallback_read(&path);
            }
        }
    }

    // Small file or non-code: read directly without truncation.
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

fn raw_fallback_read(path: &Path) -> ExitCode {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Error reading {}: {}", path.display(), e);
            return ExitCode::from(1);
        }
    };

    let total = bytes.len();
    let cap = RAW_FALLBACK_HEAD + RAW_FALLBACK_TAIL;
    let stdout = io::stdout();
    let mut out = stdout.lock();

    if total <= cap {
        let _ = out.write_all(&bytes);
    } else {
        let _ = out.write_all(&bytes[..RAW_FALLBACK_HEAD]);
        let _ = writeln!(
            out,
            "\n... [truncated, raw fallback: {} bytes omitted] ...",
            total - cap
        );
        let _ = out.write_all(&bytes[total - RAW_FALLBACK_TAIL..]);
    }
    ExitCode::SUCCESS
}

fn cmd_kw(args: &[String]) -> ExitCode {
    let path = match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("Usage: kw <path> < content");
            return ExitCode::from(2);
        }
    };

    let path = PathBuf::from(&path);

    let mut content = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut content) {
        eprintln!("Error reading stdin: {}", e);
        return ExitCode::from(1);
    }

    if let Some(parent) = path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("Error creating directory {}: {}", parent.display(), e);
                return ExitCode::from(1);
            }
        }
    }

    match std::fs::write(&path, &content) {
        Ok(()) => {
            eprintln!("Wrote {} bytes to {}", content.len(), path.display());
            observe::fire_and_forget("kw", &path.to_string_lossy(), None);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error writing {}: {}", path.display(), e);
            ExitCode::from(1)
        }
    }
}

fn cmd_ke(args: &[String]) -> ExitCode {
    let path = match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("Usage: ke <path>");
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
            eprintln!("Warning: scratchpad unreachable, allowing edit (fail-open)");
            observe::fire_and_forget("ke", &path, None);
            ExitCode::SUCCESS
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

    if p.exists() { Some(p) } else { None }
}

fn agent_forge_read(path: &Path, symbol: Option<&str>) -> Result<String, String> {
    let forge_bin = find_agent_forge().ok_or_else(|| "agent-forge binary not found".to_string())?;

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

    // Per-invocation tempfiles so concurrent kr calls don't clobber each other.
    let mut input_file = tempfile::Builder::new()
        .prefix("kleos-fs-in-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| format!("tempfile (input): {}", e))?;
    input_file
        .write_all(
            serde_json::to_string(&input_json)
                .map_err(|e| format!("serialize input: {}", e))?
                .as_bytes(),
        )
        .map_err(|e| format!("write input: {}", e))?;
    input_file
        .flush()
        .map_err(|e| format!("flush input: {}", e))?;

    let output_file = tempfile::Builder::new()
        .prefix("kleos-fs-out-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| format!("tempfile (output): {}", e))?;

    let subcommand = if symbol.is_some() {
        "search-code"
    } else {
        "repo-map"
    };

    let status = Command::new(&forge_bin)
        .arg("--input")
        .arg(input_file.path())
        .arg("--output")
        .arg(output_file.path())
        .arg(subcommand)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("spawn agent-forge: {}", e))?;

    if !status.success() {
        return Err(format!(
            "agent-forge exited with {}",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string())
        ));
    }

    let output_raw = std::fs::read_to_string(output_file.path())
        .map_err(|e| format!("read output: {}", e))?;
    let output: serde_json::Value =
        serde_json::from_str(&output_raw).map_err(|e| format!("parse output: {}", e))?;

    let success = output
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !success {
        let msg = output
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("agent-forge reported failure");
        return Err(msg.to_string());
    }

    if let Some(data) = output.get("data") {
        Ok(serde_json::to_string_pretty(data).unwrap_or_default())
    } else if let Some(msg) = output.get("message").and_then(|m| m.as_str()) {
        Ok(msg.to_string())
    } else {
        Err("agent-forge returned success with no data or message".to_string())
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

    which_in_path("agent-forge")
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

fn check_scratchpad_ledger(key: &str) -> LedgerResult {
    let server_url = env::var("KLEOS_SERVER_URL")
        .or_else(|_| env::var("ENGRAM_EIDOLON_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:4200".to_string());

    let api_key = resolve_api_key();

    let url = format!(
        "{}/scratchpad/get?namespace=spec-task&key={}",
        server_url.trim_end_matches('/'),
        urlencoded(key)
    );

    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return LedgerResult::ServerUnavailable,
    };

    let mut req = client.get(&url);
    if let Some(ref k) = api_key {
        req = req.bearer_auth(k);
    }

    let resp = match req.send() {
        Ok(r) => r,
        Err(e) => {
            // Connect/DNS/timeout/TLS -- treat as server unavailable.
            tracing_eprint(&format!("kleos-fs: scratchpad request failed: {}", e));
            return LedgerResult::ServerUnavailable;
        }
    };

    let status = resp.status();
    if status.as_u16() == 404 {
        return LedgerResult::NotFound;
    }
    if !status.is_success() {
        // Non-2xx other than 404: ambiguous, treat as unavailable so callers
        // can fail-open rather than silently blocking edits.
        return LedgerResult::ServerUnavailable;
    }

    let body = match resp.text() {
        Ok(b) => b,
        Err(_) => return LedgerResult::ServerUnavailable,
    };
    if body.trim().is_empty() || body.contains("\"value\":null") || body.contains("not found") {
        return LedgerResult::NotFound;
    }
    LedgerResult::Found
}

fn tracing_eprint(msg: &str) {
    if env::var("KLEOS_FS_DEBUG").is_ok() {
        eprintln!("{}", msg);
    }
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
