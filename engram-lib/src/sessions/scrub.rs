use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::cred::CreddClient;
use crate::db::Database;
use crate::gate::scrub_output;
use crate::Result;
use tracing::warn;

#[derive(Debug, Clone)]
struct CachedSecrets {
    secrets: Vec<String>,
    loaded_at: Instant,
}

fn scrub_cache() -> &'static Mutex<HashMap<String, CachedSecrets>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedSecrets>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub async fn scrub_message(
    db: &Database,
    credd: &CreddClient,
    user_id: i64,
    agent: &str,
    message: &str,
) -> Result<String> {
    let config = Config::from_env();
    if !config.eidolon.sessions.scrub_secrets {
        return Ok(message.to_string());
    }

    let secrets = match load_scrub_secrets(
        db,
        user_id,
        agent,
        credd,
        Duration::from_secs(config.eidolon.credd.cache_ttl_secs.max(1)),
    )
    .await
    {
        Ok(secrets) => secrets,
        Err(err) => {
            warn!(agent = %agent, user_id, error = %err, "session_secret_scrub_disabled");
            return Ok(message.to_string());
        }
    };
    Ok(apply_scrub(message, &secrets))
}

pub fn apply_scrub(message: &str, secrets: &[String]) -> String {
    scrub_output(message, secrets)
}

async fn load_scrub_secrets(
    db: &Database,
    user_id: i64,
    agent: &str,
    credd: &CreddClient,
    ttl: Duration,
) -> Result<Vec<String>> {
    // ROBUSTNESS: include the credd base URL in the cache key. The cache
    // is a process-wide static, and tests (plus prod if credd is ever
    // reconfigured) can create multiple CreddClient instances pointed at
    // different upstreams. Without the URL in the key, the first client
    // to resolve a (user_id, agent) pair poisons the cache for every
    // other client.
    let cache_key = format!(
        "scrub:{}:{user_id}:{agent}:{}",
        credd.base_url(),
        ttl.as_secs()
    );
    if let Some(cached) = cached_scrub_secrets(&cache_key, ttl) {
        return Ok(cached);
    }

    let secrets = credd.list_secret_values(db, user_id, agent).await?;
    let mut cache = scrub_cache().lock().expect("scrub cache mutex poisoned");
    cache.insert(
        cache_key,
        CachedSecrets {
            secrets: secrets.clone(),
            loaded_at: Instant::now(),
        },
    );
    Ok(secrets)
}

fn cached_scrub_secrets(cache_key: &str, ttl: Duration) -> Option<Vec<String>> {
    let cache = scrub_cache().lock().expect("scrub cache mutex poisoned");
    let entry = cache.get(cache_key)?;
    if entry.loaded_at.elapsed() > ttl {
        return None;
    }
    Some(entry.secrets.clone())
}

#[cfg(test)]
pub(crate) fn reset_scrub_cache() {
    let mut cache = scrub_cache().lock().expect("scrub cache mutex poisoned");
    cache.clear();
}

#[cfg(test)]
mod tests {
    use super::{apply_scrub, reset_scrub_cache};

    #[test]
    fn known_secret_is_redacted() {
        reset_scrub_cache();
        let result = apply_scrub(
            "The key is alpha-secret and should vanish.",
            &["alpha-secret".to_string()],
        );
        assert_eq!(result, "The key is [REDACTED] and should vanish.");
    }

    #[test]
    fn clean_message_is_preserved() {
        reset_scrub_cache();
        let input = "harmless session line";
        assert_eq!(apply_scrub(input, &["alpha-secret".to_string()]), input);
    }

    #[test]
    fn unrelated_random_text_is_preserved() {
        reset_scrub_cache();
        let corpus = [
            "fj3k2l9 test payload",
            "http://example.com/path?x=1",
            "notes about blocked-domain.com but no secret",
            "uuid 123e4567-e89b-12d3-a456-426614174000",
        ];
        for sample in corpus {
            assert_eq!(apply_scrub(sample, &["alpha-secret".to_string()]), sample);
        }
    }
}
