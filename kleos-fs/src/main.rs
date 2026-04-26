use std::env;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod observe;

const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "c", "cpp", "h", "hpp",
    "java", "rb", "swift", "kt", "scala", "zig", "hs", "ml", "ex", "exs",
    "lua", "pl", "pm", "sh", "bash", "zsh", "fish",
];

const RAW_READ_THRESHOLD: u64 = 8192;

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

fn agent_forge_read(path: &Path, symbol: Option<&str>) -> Option<String> {
    let forge_bin = find_agent_forge()?;
    let tmp_dir = env::temp_dir();
    let input_path = tmp_dir.join("kleos-fs-input.json");
    let output_path = tmp_dir.join("kleos-fs-output.json");

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
        server_url,
        urlencoded(key)
    );

    let mut cmd = Command::new("curl");
    cmd.arg("-sf")
        .arg("--max-time")
        .arg("3")
        .arg(&url);

    if let Some(ref k) = api_key {
        cmd.arg("-H").arg(format!("Authorization: Bearer {}", k));
    }

    let output = match cmd.output() {
        Ok(o) => o,
        Err(_) => return LedgerResult::ServerUnavailable,
    };

    if !output.status.success() {
        let code = output.status.code().unwrap_or(0);
        if code == 22 || code == 7 {
            return LedgerResult::ServerUnavailable;
        }
        return LedgerResult::NotFound;
    }

    let body = String::from_utf8_lossy(&output.stdout);
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
