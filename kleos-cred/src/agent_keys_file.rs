//! File-backed agent-key store for the bootstrap-bearer endpoint.
//!
//! These tokens are NOT the DB-backed `cred_agent_keys` rows used by the
//! three-tier resolve handlers; they live in a JSON file (default
//! `$XDG_CONFIG_HOME/cred/agent-keys.json`, mode 0600) so a fresh shell can
//! read them at login before any process has unlocked the cred DB.
//!
//! Each token grants a list of scopes. Bootstrap-bearer scope strings look
//! like `bootstrap/<agent-slot>`; only tokens with the matching scope can
//! call `/bootstrap/kleos-bearer?agent=<slot>`.
//!
//! Path is overridable via `CREDD_AGENT_KEYS_FILE` env (used by tests and
//! atypical installations).

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use tracing::{debug, info, warn};

/// One scoped bootstrap-agent token. Stored as 64-char hex (32 random bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAgentKey {
    /// Stable agent identifier (e.g. `<agent-name>-<host>`).
    pub id: String,
    /// 64-character hex string. Comparison is constant-time.
    pub key: String,
    pub created_at: DateTime<Utc>,
    pub last_used: Option<DateTime<Utc>>,
    pub revoked: bool,
    #[serde(default)]
    pub description: String,
    /// Scopes the token grants. Each string is `service/key` (e.g.
    /// `bootstrap/<agent-id>`), `service/*`, or `*` for full access.
    #[serde(default)]
    pub scopes: Vec<String>,
}

/// On-disk JSON shape: `{ "keys": { "<agent-id>": FileAgentKey, ... } }`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FileAgentKeyStore {
    #[serde(default)]
    pub keys: HashMap<String, FileAgentKey>,
    #[serde(skip)]
    path: PathBuf,
}

impl FileAgentKeyStore {
    /// Load from the resolved default path (env override, then XDG default).
    pub fn load() -> Result<Self> {
        Self::load_from(default_path())
    }

    /// Load from an explicit path. Empty store if the file is absent.
    /// A corrupted JSON file logs a warning and returns an empty store --
    /// credd starts cleanly so the operator can `cred agent-key generate`
    /// fresh tokens, rather than refusing to boot.
    pub fn load_from(path: PathBuf) -> Result<Self> {
        if !path.exists() {
            info!(
                "no agent-keys.json at {} -- starting with empty store",
                path.display()
            );
            return Ok(Self {
                keys: HashMap::new(),
                path,
            });
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        match serde_json::from_str::<FileAgentKeyStore>(&content) {
            Ok(mut store) => {
                store.path = path;
                info!("loaded {} bootstrap-agent key(s)", store.keys.len());
                Ok(store)
            }
            Err(e) => {
                warn!(
                    "agent-keys.json at {} is corrupted ({}); starting with empty store",
                    path.display(),
                    e
                );
                Ok(Self {
                    keys: HashMap::new(),
                    path,
                })
            }
        }
    }

    /// Atomic save: write to `<path>.tmp.<pid>` then rename. Mode 0600.
    pub fn save(&self) -> Result<()> {
        let dir = self
            .path
            .parent()
            .context("agent-keys path has no parent directory")?;
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;

        let tmp = self.path.with_extension(format!("tmp.{}", std::process::id()));
        let json = serde_json::to_string_pretty(&self)?;
        std::fs::write(&tmp, json)
            .with_context(|| format!("failed to write {}", tmp.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("failed to chmod 600 {}", tmp.display()))?;
        }

        std::fs::rename(&tmp, &self.path).with_context(|| {
            format!("failed to rename {} -> {}", tmp.display(), self.path.display())
        })?;

        debug!(
            "saved {} bootstrap-agent key(s) to {}",
            self.keys.len(),
            self.path.display()
        );
        Ok(())
    }

    /// Mint a new token. Returns the bare 64-char hex string (only chance
    /// to capture it; we store it for validation but don't surface it again).
    pub fn generate(
        &mut self,
        agent_id: &str,
        description: &str,
        scopes: Vec<String>,
    ) -> Result<String> {
        validate_agent_id(agent_id)?;
        for s in &scopes {
            validate_scope(s)?;
        }

        if let Some(existing) = self.keys.get(agent_id) {
            if !existing.revoked {
                anyhow::bail!(
                    "agent key '{}' already exists and is active; revoke it first",
                    agent_id
                );
            }
            warn!("replacing revoked key for '{}'", agent_id);
        }

        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let key_hex = hex::encode(bytes);

        let entry = FileAgentKey {
            id: agent_id.to_string(),
            key: key_hex.clone(),
            created_at: Utc::now(),
            last_used: None,
            revoked: false,
            description: description.to_string(),
            scopes,
        };
        self.keys.insert(agent_id.to_string(), entry);
        self.save()?;
        info!("minted bootstrap-agent key for '{}'", agent_id);
        Ok(key_hex)
    }

    /// Constant-time validation. Returns the agent id on match.
    pub fn validate(&self, bearer: &str) -> Option<String> {
        let token = bearer.trim();
        if token.is_empty() {
            return None;
        }
        for (id, entry) in &self.keys {
            if entry.revoked {
                continue;
            }
            if token.as_bytes().len() == entry.key.as_bytes().len()
                && token
                    .as_bytes()
                    .ct_eq(entry.key.as_bytes())
                    .unwrap_u8()
                    == 1
            {
                return Some(id.clone());
            }
        }
        None
    }

    /// Update the `last_used` timestamp. Best-effort save.
    pub fn touch(&mut self, agent_id: &str) {
        if let Some(entry) = self.keys.get_mut(agent_id) {
            entry.last_used = Some(Utc::now());
            if let Err(e) = self.save() {
                warn!("failed to persist last_used for '{}': {}", agent_id, e);
            }
        }
    }

    /// Mark a key revoked. Future validate() calls reject it.
    pub fn revoke(&mut self, agent_id: &str) -> Result<()> {
        let entry = self
            .keys
            .get_mut(agent_id)
            .ok_or_else(|| anyhow::anyhow!("agent key '{}' not found", agent_id))?;
        if entry.revoked {
            anyhow::bail!("agent key '{}' is already revoked", agent_id);
        }
        entry.revoked = true;
        self.save()?;
        info!("revoked bootstrap-agent key for '{}'", agent_id);
        Ok(())
    }

    /// Check whether the given agent has a scope covering `service/key`.
    /// Wildcard rules: exact match, `service/*`, or `*`.
    pub fn has_scope(&self, agent_id: &str, service: &str, key: &str) -> bool {
        let entry = match self.keys.get(agent_id) {
            Some(e) if !e.revoked => e,
            _ => return false,
        };
        let exact = format!("{}/{}", service, key);
        let wildcard = format!("{}/*", service);
        entry
            .scopes
            .iter()
            .any(|s| s == &exact || s == &wildcard || s == "*")
    }

    /// Get scopes for an agent (used to enrich AuthInfo::BootstrapAgent).
    pub fn scopes_for(&self, agent_id: &str) -> Vec<String> {
        self.keys
            .get(agent_id)
            .map(|k| k.scopes.clone())
            .unwrap_or_default()
    }

    pub fn list(&self) -> Vec<&FileAgentKey> {
        let mut keys: Vec<&FileAgentKey> = self.keys.values().collect();
        keys.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        keys
    }
}

/// Resolve the agent-keys.json path. `CREDD_AGENT_KEYS_FILE` env wins;
/// otherwise `$XDG_CONFIG_HOME/cred/agent-keys.json` (XDG-resolved).
pub fn default_path() -> PathBuf {
    if let Ok(p) = std::env::var("CREDD_AGENT_KEYS_FILE") {
        return PathBuf::from(p);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("."))
        });
    base.join("cred").join("agent-keys.json")
}

fn validate_agent_id(agent_id: &str) -> Result<()> {
    if agent_id.is_empty() {
        anyhow::bail!("agent_id cannot be empty");
    }
    if !agent_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "agent_id '{}' must be ascii alphanumeric, hyphens, or underscores only",
            agent_id
        );
    }
    Ok(())
}

fn validate_scope(scope: &str) -> Result<()> {
    if scope == "*" {
        return Ok(());
    }
    let parts: Vec<&str> = scope.splitn(2, '/').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        anyhow::bail!(
            "invalid scope '{}': must be '*', 'service/*', or 'service/key'",
            scope
        );
    }
    let valid_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.';
    if !parts[0].bytes().all(valid_byte) {
        anyhow::bail!("invalid scope '{}': service name has invalid characters", scope);
    }
    if parts[1] != "*" && !parts[1].bytes().all(valid_byte) {
        anyhow::bail!("invalid scope '{}': key name has invalid characters", scope);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn generate_validate_roundtrip() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-keys.json");
        let mut store = FileAgentKeyStore::load_from(path.clone()).unwrap();

        let token = store
            .generate(
                "test-agent-1",
                "test",
                vec!["bootstrap/test-agent-1".into()],
            )
            .unwrap();
        assert_eq!(token.len(), 64);

        let validated = store.validate(&token).expect("must validate");
        assert_eq!(validated, "test-agent-1");

        // Reload from disk -- atomic write should produce a parseable file.
        let reloaded = FileAgentKeyStore::load_from(path).unwrap();
        assert!(reloaded.has_scope("test-agent-1", "bootstrap", "test-agent-1"));
    }

    #[test]
    fn revoked_token_rejected() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-keys.json");
        let mut store = FileAgentKeyStore::load_from(path).unwrap();
        let token = store
            .generate("agent-x", "", vec!["bootstrap/agent-x".into()])
            .unwrap();
        store.revoke("agent-x").unwrap();
        assert_eq!(store.validate(&token), None);
    }

    #[test]
    fn scope_wildcards() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-keys.json");
        let mut store = FileAgentKeyStore::load_from(path).unwrap();
        store
            .generate(
                "agent-y",
                "",
                vec!["bootstrap/*".into(), "engram-rust/foo".into()],
            )
            .unwrap();
        assert!(store.has_scope("agent-y", "bootstrap", "anything"));
        assert!(store.has_scope("agent-y", "engram-rust", "foo"));
        assert!(!store.has_scope("agent-y", "engram-rust", "bar"));
    }

    #[test]
    fn invalid_scope_rejected() {
        let _g = ENV_GUARD.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-keys.json");
        let mut store = FileAgentKeyStore::load_from(path).unwrap();
        let r = store.generate("z", "", vec!["bad scope with spaces".into()]);
        assert!(r.is_err());
    }
}
