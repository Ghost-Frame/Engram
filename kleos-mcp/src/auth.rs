use crate::App;
use kleos_lib::auth::{validate_key, AuthContext, Scope};
use kleos_lib::{EngError, Result};
use serde_json::Value;

/// Resolve authentication from the JSON-RPC arguments.
///
/// Precedence (highest first):
///   1. `bearer_token` field in the JSON-RPC `args` object.
///   2. `ENGRAM_MCP_BEARER_TOKEN` environment variable. The
///      `KLEOS_MCP_BEARER_TOKEN` alias is honoured automatically because
///      [`kleos_lib::config::migrate_env_prefix`] mirrors `KLEOS_*` to
///      `ENGRAM_*` at process startup.
///
/// For **stdio** transport the env-var fallback is the supported way to wire
/// auth -- stdio has no per-call header.
///
/// For **HTTP** transport, transport-level middleware already validates the
/// bearer token before any handler runs. The per-tool `bearer_token` field
/// and the env fallback are still honoured so an HTTP caller can downgrade
/// to a scoped key without re-running the transport handshake.
///
/// SECURITY (SEC-LOW-3): callers that log `args` MUST strip the
/// `bearer_token` field first. [`sanitize_args`] is provided for this.
#[tracing::instrument(skip(app, args))]
pub async fn resolve_auth(app: &App, args: &Value) -> Result<AuthContext> {
    let token = resolve_bearer_token(args).ok_or_else(|| {
        EngError::Auth(
            "missing bearer_token argument and ENGRAM_MCP_BEARER_TOKEN env var is unset".into(),
        )
    })?;
    validate_key(&app.db, &token).await
}

/// Pull the bearer token out of `args`, falling back to the
/// `ENGRAM_MCP_BEARER_TOKEN` env var (and its `KLEOS_MCP_BEARER_TOKEN`
/// alias, which is mirrored at startup). Empty strings are treated as
/// absent.
fn resolve_bearer_token(args: &Value) -> Option<String> {
    if let Some(token) = args.get("bearer_token").and_then(Value::as_str) {
        if !token.is_empty() {
            return Some(token.to_string());
        }
    }
    if let Ok(token) = std::env::var("ENGRAM_MCP_BEARER_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }
    None
}

/// Return a copy of `args` with `bearer_token` redacted for safe logging.
pub fn sanitize_args(args: &Value) -> Value {
    let mut clean = args.clone();
    if let Some(obj) = clean.as_object_mut() {
        if obj.contains_key("bearer_token") {
            obj.insert("bearer_token".into(), Value::String("[REDACTED]".into()));
        }
    }
    clean
}

pub fn require_admin(auth: &AuthContext) -> Result<()> {
    if auth.has_scope(&Scope::Admin) {
        Ok(())
    } else {
        Err(EngError::Auth("admin scope required".into()))
    }
}

/// Require a write-capable scope (Write or Admin).
///
/// SECURITY (R7-001): MCP tools that mutate state must call this before
/// performing writes. `has_scope(&Write)` treats Admin as a superset of Write,
/// so admin-scoped keys pass.
pub fn require_write(auth: &AuthContext) -> Result<()> {
    if auth.has_scope(&Scope::Write) {
        Ok(())
    } else {
        Err(EngError::Auth("write scope required".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_bearer_token;
    use serde_json::json;

    /// Lock around env-var manipulation: tests run in parallel and the env
    /// is process-global, so concurrent set/remove calls would race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<F: FnOnce()>(set: Option<&str>, body: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("ENGRAM_MCP_BEARER_TOKEN").ok();
        match set {
            Some(v) => std::env::set_var("ENGRAM_MCP_BEARER_TOKEN", v),
            None => std::env::remove_var("ENGRAM_MCP_BEARER_TOKEN"),
        }
        body();
        match prev {
            Some(v) => std::env::set_var("ENGRAM_MCP_BEARER_TOKEN", v),
            None => std::env::remove_var("ENGRAM_MCP_BEARER_TOKEN"),
        }
    }

    #[test]
    fn arg_bearer_token_wins_over_env() {
        with_env(Some("env-token"), || {
            let args = json!({ "bearer_token": "arg-token" });
            assert_eq!(resolve_bearer_token(&args).as_deref(), Some("arg-token"));
        });
    }

    #[test]
    fn env_fallback_used_when_arg_absent() {
        with_env(Some("env-token"), || {
            let args = json!({});
            assert_eq!(resolve_bearer_token(&args).as_deref(), Some("env-token"));
        });
    }

    #[test]
    fn env_fallback_used_when_arg_is_empty_string() {
        with_env(Some("env-token"), || {
            let args = json!({ "bearer_token": "" });
            assert_eq!(resolve_bearer_token(&args).as_deref(), Some("env-token"));
        });
    }

    #[test]
    fn env_fallback_used_when_arg_is_not_a_string() {
        with_env(Some("env-token"), || {
            let args = json!({ "bearer_token": 12345 });
            assert_eq!(resolve_bearer_token(&args).as_deref(), Some("env-token"));
        });
    }

    #[test]
    fn returns_none_when_arg_and_env_both_absent() {
        with_env(None, || {
            let args = json!({});
            assert!(resolve_bearer_token(&args).is_none());
        });
    }

    #[test]
    fn returns_none_when_arg_empty_and_env_empty() {
        with_env(Some(""), || {
            let args = json!({ "bearer_token": "" });
            assert!(resolve_bearer_token(&args).is_none());
        });
    }

    /// `migrate_env_prefix` runs at binary startup and mirrors
    /// `KLEOS_MCP_BEARER_TOKEN` into `ENGRAM_MCP_BEARER_TOKEN`. We invoke it
    /// here to prove the alias works for the documented stdio flow.
    #[test]
    fn kleos_alias_env_via_migrate() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_kleos = std::env::var("KLEOS_MCP_BEARER_TOKEN").ok();
        let prev_engram = std::env::var("ENGRAM_MCP_BEARER_TOKEN").ok();

        std::env::remove_var("ENGRAM_MCP_BEARER_TOKEN");
        std::env::set_var("KLEOS_MCP_BEARER_TOKEN", "kleos-aliased");
        kleos_lib::config::migrate_env_prefix();

        let args = json!({});
        assert_eq!(
            resolve_bearer_token(&args).as_deref(),
            Some("kleos-aliased")
        );

        match prev_kleos {
            Some(v) => std::env::set_var("KLEOS_MCP_BEARER_TOKEN", v),
            None => std::env::remove_var("KLEOS_MCP_BEARER_TOKEN"),
        }
        match prev_engram {
            Some(v) => std::env::set_var("ENGRAM_MCP_BEARER_TOKEN", v),
            None => std::env::remove_var("ENGRAM_MCP_BEARER_TOKEN"),
        }
    }
}
