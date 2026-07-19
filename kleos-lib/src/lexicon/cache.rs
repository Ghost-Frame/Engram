// ============================================================================
// Lexicon -- runtime override cache.
//
// Mirrors the pattern used by `kleos-lib/src/llm/prompts.rs` for prompt
// overrides:
//
// - Cascade for the override repo path:
//     1. KLEOS_LEXICON_REPOSITORY (explicit, any directory)
//     2. KLEOS_DATA_DIR/lexicon or ENGRAM_DATA_DIR/lexicon (if existing)
//     3. None -> embedded-only mode (zero I/O on the hot path)
//
// - Cache entries hold `Option<Arc<ParsedLexicon>>` keyed by language code.
//   TTL is 5 seconds; reload is triggered when the file's mtime differs
//   from the cached mtime. Missed lookups also cache the negative result.
//
// - Concurrency: RwLock<HashMap<...>> + OnceLock initialisation. Multiple
//   readers serialize only on the slow path (revalidation after TTL).
// ============================================================================

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

use super::loader::{parse, ParsedLexicon};

/// How long a cached entry stays trusted without re-checking the filesystem.
/// Same value as `llm::prompts::TTL_SECS` for consistency.
const TTL_SECS: u64 = 5;

struct CacheEntry {
    /// Parsed lexicon as currently known. `None` means we resolved a miss
    /// (file not present, parse error, or unreadable) and should keep
    /// falling back until the TTL expires.
    parsed: Option<Arc<ParsedLexicon>>,
    /// `mtime` observed for `parsed`. `None` together with `parsed == None`
    /// signals a confirmed miss; otherwise tracks the file modification
    /// time used to decide whether a reload is needed.
    mtime: Option<SystemTime>,
    /// Wall-clock instant at which the entry was last checked. Used to
    /// throttle filesystem access via `TTL_SECS`.
    checked_at: Instant,
}

fn cache() -> &'static RwLock<HashMap<String, CacheEntry>> {
    static CACHE: OnceLock<RwLock<HashMap<String, CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Resolve the override repo path once per process. Returns `None` if no
/// override directory is configured (embedded-only mode).
pub(super) fn repo_root() -> Option<&'static PathBuf> {
    static REPO: OnceLock<Option<PathBuf>> = OnceLock::new();
    REPO.get_or_init(|| {
        // 1. Explicit override: KLEOS_LEXICON_REPOSITORY (any directory).
        if let Some(raw) = std::env::var_os("KLEOS_LEXICON_REPOSITORY") {
            let p = PathBuf::from(raw);
            if !p.as_os_str().is_empty() {
                return Some(p);
            }
        }
        // 2. Implicit fallback: <KLEOS_DATA_DIR>/lexicon when the convention
        //    directory exists. Mirrors the prompts.rs convention by reading
        //    both KLEOS_* and ENGRAM_* (config::migrate_env_prefix maps them).
        for env in ["KLEOS_DATA_DIR", "ENGRAM_DATA_DIR"] {
            if let Some(raw) = std::env::var_os(env) {
                let candidate = PathBuf::from(raw).join("lexicon");
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
        None
    })
    .as_ref()
}

fn path_for(repo: &Path, lang: &str) -> PathBuf {
    repo.join(format!("{lang}.toml"))
}

/// Look up the parsed override for `lang`. Returns `None` if no override
/// file exists (caller should fall back to the embedded baseline).
pub(super) fn resolve_override(repo: &Path, lang: &str) -> Option<Arc<ParsedLexicon>> {
    // Fast path: cache hit within TTL window.
    {
        let cache_g = cache().read().ok()?;
        if let Some(entry) = cache_g.get(lang) {
            if entry.checked_at.elapsed() < Duration::from_secs(TTL_SECS) {
                return entry.parsed.as_ref().map(Arc::clone);
            }
        }
    }

    // Slow path: revalidate against the filesystem.
    let path = path_for(repo, lang);
    let fresh_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let now = Instant::now();

    // If the file is still there and unchanged, reuse the cached parsed
    // value without re-reading + re-parsing.
    {
        let cache_g = cache().read().ok()?;
        if let Some(entry) = cache_g.get(lang) {
            if entry.mtime == fresh_mtime && entry.parsed.is_some() {
                drop(cache_g);
                let mut w = cache().write().ok()?;
                if let Some(e) = w.get_mut(lang) {
                    e.checked_at = now;
                }
                return w.get(lang).and_then(|e| e.parsed.as_ref().map(Arc::clone));
            }
        }
    }

    // Mtime missing or different: re-read + re-parse.
    let new_parsed = match std::fs::read_to_string(&path) {
        Ok(src) => match parse(&src) {
            Ok(parsed) => Some(Arc::new(parsed)),
            Err(e) => {
                tracing::warn!(
                    lang = lang,
                    path = %path.display(),
                    error = %e,
                    "lexicon override parse failed, falling back to embedded baseline",
                );
                None
            }
        },
        Err(_) => {
            // Distinguish a freshly missing file from a never-present one,
            // mirroring prompts.rs.
            let lost_existing = cache()
                .read()
                .ok()
                .and_then(|g| g.get(lang).map(|e| e.parsed.is_some()))
                .unwrap_or(false);
            if lost_existing {
                tracing::warn!(
                    lang = lang,
                    path = %path.display(),
                    "lexicon override file disappeared, falling back to embedded baseline",
                );
            } else {
                tracing::debug!(
                    lang = lang,
                    path = %path.display(),
                    "no lexicon override file (using embedded baseline)",
                );
            }
            None
        }
    };

    let mut w = cache().write().ok()?;
    let entry = CacheEntry {
        parsed: new_parsed.clone(),
        mtime: fresh_mtime,
        checked_at: now,
    };
    w.insert(lang.to_string(), entry);
    new_parsed
}

#[cfg(test)]
pub(super) fn clear_cache() {
    if let Ok(mut g) = cache().write() {
        g.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;

    // Serialize the few env-mutating tests in this module.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn make_tempdir(label: &str) -> PathBuf {
        let nano = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("kleos-lexicon-test-{label}-{nano}"));
        fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    #[test]
    fn resolve_override_misses_when_file_absent() {
        let _g = env_lock().lock().unwrap();
        clear_cache();
        let dir = make_tempdir("miss");
        assert!(resolve_override(&dir, "zz").is_none());
    }

    #[test]
    fn resolve_override_hits_existing_file() {
        let _g = env_lock().lock().unwrap();
        clear_cache();
        let dir = make_tempdir("hit");
        let path = dir.join("xx.toml");
        fs::write(
            &path,
            r#"
schema_version = 1
language = "xx"

[classes.verb_like]
words = ["xx_love", "xx_like"]
"#,
        )
        .unwrap();
        let got = resolve_override(&dir, "xx").expect("override should load");
        let verb_like = got.classes.get("verb_like").expect("verb_like present");
        assert_eq!(
            verb_like.words,
            vec!["xx_love".to_string(), "xx_like".to_string()]
        );
    }

    #[test]
    fn resolve_override_caches_negative_result() {
        let _g = env_lock().lock().unwrap();
        clear_cache();
        let dir = make_tempdir("neg");
        assert!(resolve_override(&dir, "yy").is_none());
        // Second call within TTL must also miss (negative cache).
        assert!(resolve_override(&dir, "yy").is_none());
    }

    #[test]
    fn resolve_override_invalid_toml_falls_back() {
        let _g = env_lock().lock().unwrap();
        clear_cache();
        let dir = make_tempdir("bad");
        let path = dir.join("zz.toml");
        fs::write(&path, "this is [[[ not toml").unwrap();
        let got = resolve_override(&dir, "zz");
        assert!(got.is_none(), "parse failure should fall back to None");
    }

    #[test]
    fn resolve_override_reloads_after_mtime_change() {
        let _g = env_lock().lock().unwrap();
        clear_cache();
        let dir = make_tempdir("mtime");
        let path = dir.join("ww.toml");
        fs::write(
            &path,
            r#"
schema_version = 1
language = "ww"

[classes.verb_like]
words = ["v1"]
"#,
        )
        .unwrap();
        let first = resolve_override(&dir, "ww").expect("v1");
        assert_eq!(
            first.classes.get("verb_like").unwrap().words,
            vec!["v1".to_string()]
        );

        // Wait past the TTL window then rewrite with a different mtime.
        std::thread::sleep(Duration::from_millis(TTL_SECS * 1000 + 200));
        fs::write(
            &path,
            r#"
schema_version = 1
language = "ww"

[classes.verb_like]
words = ["v2"]
"#,
        )
        .unwrap();
        let second = resolve_override(&dir, "ww").expect("v2");
        assert_eq!(
            second.classes.get("verb_like").unwrap().words,
            vec!["v2".to_string()]
        );
    }
}
