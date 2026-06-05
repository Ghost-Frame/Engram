//! Integration tests for Phylax agent-native credential features.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use rusqlite::params;
use serde_json::{json, Value};
use tower::ServiceExt;

use kleos_cred::audit::{self, AccessTier, AuditAction};
use kleos_cred::crypto::derive_key;
use kleos_cred::storage::store_secret;
use kleos_cred::types::SecretData;
use kleos_credd::state::AppState;
use kleos_lib::db::Database;
use kleos_lib::EngError;
use kleos_phylax::audit::{actions, log_phylax_audit};
use kleos_phylax::models::ssh_settings::{list_ssh_settings, upsert_ssh_settings};
use kleos_phylax::router::compose_router;

/// Test harness wrapping a Phylax-enabled router.
struct TestApp {
    /// Combined credd + phylax router with all middleware applied.
    router: Router,
    /// Shared test database.
    db: std::sync::Arc<Database>,
    /// Hex-encoded master token derived from the test password.
    master_token: String,
}

/// Helpers for building and exercising a Phylax-aware test app.
impl TestApp {
    /// Create a test app with in-memory DB, migrations applied.
    async fn new() -> Self {
        let db = Database::connect_memory().await.expect("in-memory db");
        let master_password = "test-master-password";
        let master_key = derive_key(1, master_password.as_bytes(), None);
        let master_token = hex::encode(*master_key);

        let app_state = AppState::new(db, *master_key);

        // Compose base credd routes with phylax extensions and shared policy
        // middleware ordering.
        let router = compose_router(app_state.clone());

        Self {
            router,
            db: app_state.db.clone(),
            master_token,
        }
    }

    /// Send an authenticated request with the master token.
    async fn request_master(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.request_auth(method, path, body, &self.master_token.clone())
            .await
    }

    /// Send an authenticated request with a specific token.
    async fn request_auth(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        token: &str,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {}", token));

        let body = if let Some(json) = body {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&json).unwrap())
        } else {
            Body::empty()
        };

        let req = builder.body(body).unwrap();
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let body_bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json: Value = if body_bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body_bytes).unwrap_or(Value::Null)
        };
        (status, json)
    }
}

// ---- Policy CRUD tests ----

/// Test full create/read/update/delete lifecycle for access policies.
#[tokio::test]
async fn test_policy_crud() {
    let app = TestApp::new().await;

    // List policies -- should be empty.
    let (status, body) = app.request_master("GET", "/phylax/policies", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["policies"].as_array().unwrap().len(), 0);

    // Create a policy.
    let (status, body) = app
        .request_master(
            "POST",
            "/phylax/policies",
            Some(json!({
                "namespace": "default",
                "category": "prod",
                "require_approval": true,
                "allowed_modes": ["text", "proxy"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let policy_id = body["id"].as_i64().unwrap();
    assert!(policy_id > 0);
    assert_eq!(body["namespace"], "default");
    assert_eq!(body["require_approval"], true);

    // List policies -- should have one.
    let (status, body) = app.request_master("GET", "/phylax/policies", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["policies"].as_array().unwrap().len(), 1);

    // Update the policy.
    let (status, _) = app
        .request_master(
            "PUT",
            &format!("/phylax/policies/{}", policy_id),
            Some(json!({
                "require_approval": false,
                "allowed_modes": ["text"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Delete the policy.
    let (status, _) = app
        .request_master("DELETE", &format!("/phylax/policies/{}", policy_id), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    // List policies -- should be empty again.
    let (status, body) = app.request_master("GET", "/phylax/policies", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["policies"].as_array().unwrap().len(), 0);
}

// ---- Approval flow tests ----

/// Test full approval flow: agent requests, master approves, lease is minted.
#[tokio::test]
async fn test_approval_flow() {
    let app = TestApp::new().await;

    // Create a policy requiring approval for prod secrets.
    let (status, _) = app
        .request_master(
            "POST",
            "/phylax/policies",
            Some(json!({
                "namespace": "default",
                "category": "prod",
                "require_approval": true
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Create an agent key for testing.
    let (status, body) = app
        .request_master(
            "POST",
            "/agents",
            Some(json!({
                "name": "test-agent",
                "categories": ["prod/*"],
                "allow_raw": false
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let agent_key = body["key"].as_str().unwrap().to_string();

    // Agent requests approval.
    let (status, body) = app
        .request_auth(
            "POST",
            "/phylax/approvals",
            Some(json!({
                "category": "prod",
                "secret_name": "deploy-key",
                "resolve_mode": "text"
            })),
            &agent_key,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let approval_id = body["approval_id"].as_i64().unwrap();
    assert!(approval_id > 0);
    assert!(body["poll_url"]
        .as_str()
        .unwrap()
        .contains(&approval_id.to_string()));

    // Get approval status -- should be pending (status=0).
    let (status, body) = app
        .request_master("GET", &format!("/phylax/approvals/{}", approval_id), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], 0);

    // Approve it.
    let (status, body) = app
        .request_master(
            "PUT",
            &format!("/phylax/approvals/{}", approval_id),
            Some(json!({
                "decision": "approved",
                "reason": "test approval"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "approved");
    assert!(body["lease"]["jti"].is_string());

    // List leases -- should have at least one active lease.
    let (status, body) = app.request_master("GET", "/phylax/leases", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["leases"].as_array().unwrap().is_empty());
}

/// Test that policy-gated resolve endpoints return approvals for agents.
#[tokio::test]
async fn test_resolve_raw_requires_approval() {
    let app = TestApp::new().await;

    let (status, _) = app
        .request_master(
            "POST",
            "/secret/prod/db-pass",
            Some(json!({
                "data": {
                    "type": "note",
                    "content": "super-secret"
                }
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = app
        .request_master(
            "POST",
            "/phylax/policies",
            Some(json!({
                "namespace": "default",
                "category": "prod",
                "require_approval": true,
                "allowed_modes": ["raw"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = app
        .request_master(
            "POST",
            "/agents",
            Some(json!({
                "name": "raw-agent",
                "categories": ["prod/*"],
                "allow_raw": true
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let agent_key = body["key"].as_str().unwrap().to_string();

    let (status, body) = app
        .request_auth(
            "POST",
            "/resolve/raw",
            Some(json!({
                "category": "prod",
                "name": "db-pass"
            })),
            &agent_key,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(body["approval_required"], true);
    assert!(body["approval_id"].as_i64().is_some());
}

// ---- Approval denial test ----

/// Test that an approval can be denied and returns denied status.
#[tokio::test]
async fn test_approval_denial() {
    let app = TestApp::new().await;

    // Create an agent key.
    let (status, body) = app
        .request_master(
            "POST",
            "/agents",
            Some(json!({
                "name": "deny-agent",
                "categories": ["prod/*"],
                "allow_raw": false
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let agent_key = body["key"].as_str().unwrap().to_string();

    // Agent requests approval.
    let (status, body) = app
        .request_auth(
            "POST",
            "/phylax/approvals",
            Some(json!({
                "category": "prod",
                "secret_name": "secret-key",
                "resolve_mode": "text"
            })),
            &agent_key,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let approval_id = body["approval_id"].as_i64().unwrap();

    // Deny it.
    let (status, body) = app
        .request_master(
            "PUT",
            &format!("/phylax/approvals/{}", approval_id),
            Some(json!({
                "decision": "denied",
                "reason": "test denial"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "denied");
}

// ---- Namespace enumeration test ----

/// Test that policies created in different namespaces are all visible via
/// the namespace enumeration endpoint.
#[tokio::test]
async fn test_namespace_list() {
    let app = TestApp::new().await;

    // Create policies in different namespaces.
    for ns in &["alpha", "beta", "gamma"] {
        let (status, _) = app
            .request_master(
                "POST",
                "/phylax/policies",
                Some(json!({
                    "namespace": ns,
                    "require_approval": true
                })),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }

    // List namespaces.
    let (status, body) = app.request_master("GET", "/phylax/namespaces", None).await;
    assert_eq!(status, StatusCode::OK);
    let namespaces = body["namespaces"].as_array().unwrap();
    assert_eq!(namespaces.len(), 3);
    assert!(namespaces.iter().any(|n| n == "alpha"));
    assert!(namespaces.iter().any(|n| n == "beta"));
    assert!(namespaces.iter().any(|n| n == "gamma"));
}

// ---- ECDH challenge test ----

/// Test that an ECDH challenge can be issued and returns a valid 32-byte nonce.
#[tokio::test]
async fn test_ecdh_challenge() {
    let app = TestApp::new().await;

    // Issue a challenge.
    let (status, body) = app
        .request_master("POST", "/phylax/ecdh/challenge", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["challenge_id"].is_string());
    assert!(body["nonce"].is_string());
    // Nonce should be 64 hex chars (32 bytes).
    assert_eq!(body["nonce"].as_str().unwrap().len(), 64);
}

// ---- SSH settings test ----

/// Test get/update lifecycle for SSH key settings on a specific secret.
#[tokio::test]
async fn test_ssh_settings() {
    let app = TestApp::new().await;

    // Get default settings (no settings configured yet).
    let (status, body) = app
        .request_master("GET", "/phylax/ssh/ssh-keys/deploy", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["auto_sign"], false);
    assert_eq!(body["auto_load"], false);

    // Update settings.
    let (status, body) = app
        .request_master(
            "PUT",
            "/phylax/ssh/ssh-keys/deploy",
            Some(json!({
                "auto_sign": true,
                "auto_load": true
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["auto_sign"], true);
    assert_eq!(body["auto_load"], true);

    // Get updated settings -- should reflect the change.
    let (status, body) = app
        .request_master("GET", "/phylax/ssh/ssh-keys/deploy", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["auto_sign"], true);
    assert_eq!(body["auto_load"], true);
}

// ---- PIV enrollment test ----

/// Test PIV public key enrollment and revocation, including idempotency
/// check (double-revoke must fail).
#[tokio::test]
async fn test_piv_enroll_revoke() {
    let app = TestApp::new().await;

    // Enroll a pubkey.
    let (status, body) = app
        .request_master(
            "POST",
            "/phylax/ecdh/enroll",
            Some(json!({
                "agent_name": "test-agent",
                "public_key_pem": "-----BEGIN PUBLIC KEY-----\ntest\n-----END PUBLIC KEY-----"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let pubkey_id = body["id"].as_i64().unwrap();
    assert!(pubkey_id > 0);

    // Revoke it.
    let (status, _) = app
        .request_master(
            "POST",
            "/phylax/ecdh/revoke",
            Some(json!({
                "pubkey_id": pubkey_id
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    // Revoking again should fail (already revoked).
    let (status, _) = app
        .request_master(
            "POST",
            "/phylax/ecdh/revoke",
            Some(json!({
                "pubkey_id": pubkey_id
            })),
        )
        .await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
/// Verify legacy audit insertion still works with the new nullable columns present.
async fn test_log_audit_compatible_after_cred_audit_extension() {
    let app = TestApp::new().await;

    let audit_id = audit::log_audit(
        &app.db,
        1,
        Some("agent-legacy"),
        AuditAction::Get,
        "prod",
        "api-key",
        Some(AccessTier::Proxy),
        true,
    )
    .await
    .unwrap();

    let (operator_id, source_ip, policy_id, session_id): (Option<String>, Option<String>, Option<i64>, Option<String>) =
        app.db
            .read(move |conn| {
                conn.query_row(
                    "SELECT operator_id, source_ip, policy_id, session_id FROM cred_audit WHERE id = ?1",
                    params![audit_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                        ))
                    },
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
            })
            .await
            .unwrap();

    assert!(operator_id.is_none());
    assert!(source_ip.is_none());
    assert!(policy_id.is_none());
    assert!(session_id.is_none());
}

#[tokio::test]
/// Verify Phylax audit can write operator/source/policy/session metadata columns.
async fn test_phylax_audit_writes_attribution_columns() {
    let app = TestApp::new().await;

    let id = log_phylax_audit(
        &app.db,
        1,
        Some("agent-1"),
        Some("operator-1"),
        Some("127.0.0.1"),
        Some(42),
        Some("session-1"),
        actions::LEASE_MINTED,
        "prod",
        "api-key",
        true,
        Some("corr-1"),
    )
    .await
    .unwrap();

    let (operator_id, source_ip, policy_id, session_id): (Option<String>, Option<String>, Option<i64>, Option<String>) =
        app.db
            .read(move |conn| {
                conn.query_row(
                    "SELECT operator_id, source_ip, policy_id, session_id FROM cred_audit WHERE id = ?1",
                    params![id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                        ))
                    },
                )
                .map_err(|e| EngError::DatabaseMessage(e.to_string()))
            })
            .await
            .unwrap();

    assert_eq!(operator_id.as_deref(), Some("operator-1"));
    assert_eq!(source_ip.as_deref(), Some("127.0.0.1"));
    assert_eq!(policy_id, Some(42));
    assert_eq!(session_id.as_deref(), Some("session-1"));
}

#[tokio::test]
/// Verify lease redemption consumes approval without returning the secret value.
async fn test_redeem_lease_does_not_return_plaintext_secret() {
    let app = TestApp::new().await;

    let (status, _body) = app
        .request_master(
            "POST",
            "/phylax/policies",
            Some(json!({
                "namespace": "default",
                "category": "prod",
                "require_approval": true,
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = app
        .request_auth(
            "POST",
            "/agents",
            Some(json!({
                "name": "redeem-agent",
                "categories": ["prod/*"],
                "allow_raw": false
            })),
            &app.master_token,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let agent_key = body["key"].as_str().unwrap().to_string();

    let (status, _) = app
        .request_master(
            "POST",
            "/secret/prod/secret-key",
            Some(json!({
                "data": { "type": "api_key", "key": "super-secret-key" }
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = app
        .request_auth(
            "POST",
            "/phylax/approvals",
            Some(json!({
                "category": "prod",
                "secret_name": "secret-key",
                "resolve_mode": "text"
            })),
            &agent_key,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let approval_id = body["approval_id"].as_i64().unwrap();

    let (status, body) = app
        .request_master(
            "PUT",
            &format!("/phylax/approvals/{}", approval_id),
            Some(json!({
                "decision": "approved",
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let jti = body["lease"]["jti"].as_str().unwrap().to_string();

    let (status, body) = app
        .request_auth(
            "POST",
            &format!("/phylax/leases/{}/redeem", jti),
            None,
            &agent_key,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "redeemed");
    assert!(body["secret"].is_null());
    assert_eq!(
        body["message"],
        "plaintext delivery disabled until proxy delivery is enabled"
    );
    assert!(
        !body.to_string().contains("super-secret-key"),
        "redeemed lease response must not contain the secret value"
    );

    let (status, body) = app
        .request_auth(
            "POST",
            &format!("/phylax/leases/{}/redeem", jti),
            None,
            &agent_key,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "lease already redeemed");
}

// ---- list_ssh_settings model test ----

/// Verify list_ssh_settings returns all rows for a user, ordered by
/// category then secret_name, and excludes rows for other users.
#[tokio::test]
async fn test_list_ssh_settings_returns_all_rows_for_user() {
    let app = TestApp::new().await;

    // user_id=1 is the master; insert two settings rows via upsert.
    upsert_ssh_settings(&app.db, 1, "ssh-keys", "deploy", true, false)
        .await
        .unwrap();
    upsert_ssh_settings(&app.db, 1, "infra", "bastion", false, true)
        .await
        .unwrap();
    // Row for a different user -- must not appear in user_id=1 results.
    upsert_ssh_settings(&app.db, 99, "ssh-keys", "other", false, false)
        .await
        .unwrap();

    let rows = list_ssh_settings(&app.db, 1).await.unwrap();

    // Should be ordered category ASC, secret_name ASC: infra/bastion then ssh-keys/deploy.
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].category, "infra");
    assert_eq!(rows[0].secret_name, "bastion");
    assert!(!rows[0].auto_sign);
    assert!(rows[0].auto_load);

    assert_eq!(rows[1].category, "ssh-keys");
    assert_eq!(rows[1].secret_name, "deploy");
    assert!(rows[1].auto_sign);
    assert!(!rows[1].auto_load);
}

// ---- SSH sign + identities HTTP integration tests ----

/// Happy-path sign: generate an ephemeral ed25519 key, seed it with auto_sign=true,
/// POST to the sign endpoint, assert HTTP 200 + non-empty signature_hex, then
/// cryptographically verify the signature against the generated public key.
#[tokio::test]
async fn test_ssh_sign_auto_sign_true_returns_verified_signature() {
    // Must be in scope for `public_key.verify(...)` to resolve.
    use signature::Verifier;

    // Generate a fresh ephemeral key for this test.
    // ssh-key 0.6 requires rand_core 0.6.x; rand 0.9 (workspace) uses rand_core 0.9,
    // so we pull rand_core 0.6 directly as a dev-dependency.
    let key =
        ssh_key::private::PrivateKey::random(&mut rand_core::OsRng, ssh_key::Algorithm::Ed25519)
            .expect("ephemeral ed25519 key generation must succeed");
    let pem = key
        .to_openssh(ssh_key::LineEnding::LF)
        .expect("private key must encode to OpenSSH PEM");
    let public_key = key.public_key().clone();
    // Encode public key in OpenSSH authorized_keys format for storage.
    let public_key_str = public_key
        .to_openssh()
        .expect("public key must encode to OpenSSH");

    let app = TestApp::new().await;
    let master_key = derive_key(1, "test-master-password".as_bytes(), None);

    // Seed the SSH key secret into the vault (user_id=1 is the master).
    store_secret(
        &app.db,
        1,
        "ssh-keys",
        "deploy",
        &SecretData::SshKey {
            private_key: pem.to_string(),
            public_key: Some(public_key_str.trim().to_string()),
            passphrase: None,
        },
        &master_key,
    )
    .await
    .expect("store_secret must succeed");

    // Mark the key as auto_sign=true so no approval gate is triggered.
    upsert_ssh_settings(&app.db, 1, "ssh-keys", "deploy", true, false)
        .await
        .expect("upsert_ssh_settings must succeed");

    // Bytes to sign -- use a short challenge.
    let challenge = b"http-integration-challenge";
    let data_hex = hex::encode(challenge);

    // POST /phylax/ssh/ssh-keys/deploy/sign with master auth.
    let (status, body) = app
        .request_master(
            "POST",
            "/phylax/ssh/ssh-keys/deploy/sign",
            Some(json!({ "data_hex": data_hex, "flags": 0 })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "sign endpoint must return 200; body={body}"
    );

    let sig_hex = body["signature_hex"]
        .as_str()
        .expect("signature_hex must be a string");
    assert!(!sig_hex.is_empty(), "signature_hex must not be empty");

    // Decode the SSH wire-format blob and verify it cryptographically.
    let blob = hex::decode(sig_hex).expect("signature_hex must be valid hex");
    let sig = ssh_key::Signature::try_from(blob.as_slice())
        .expect("blob must decode as a valid ssh_key::Signature");

    // Call through key_data() to reach the Verifier<ssh_key::Signature> impl
    // (mirrors ssh_sign_test.rs approach).
    Verifier::verify(public_key.key_data(), challenge.as_slice(), &sig)
        .expect("signature must cryptographically verify against the generated public key");
}

/// Identities lists public-only: generate an ephemeral key, seed it with auto_sign=true,
/// GET /phylax/ssh/identities, assert the key appears with public_openssh populated
/// and auto_sign=true, and assert no private key material leaks into the response JSON.
#[tokio::test]
async fn test_ssh_identities_returns_public_material_only() {
    // Generate a fresh ephemeral key for this test -- independent of the sign test's key.
    let key =
        ssh_key::private::PrivateKey::random(&mut rand_core::OsRng, ssh_key::Algorithm::Ed25519)
            .expect("ephemeral ed25519 key generation must succeed");
    let pem = key
        .to_openssh(ssh_key::LineEnding::LF)
        .expect("private key must encode to OpenSSH PEM");
    let public_key_str = key
        .public_key()
        .to_openssh()
        .expect("public key must encode to OpenSSH");

    let app = TestApp::new().await;
    let master_key = derive_key(1, "test-master-password".as_bytes(), None);

    // Seed the key.
    store_secret(
        &app.db,
        1,
        "ssh-keys",
        "id-test",
        &SecretData::SshKey {
            private_key: pem.to_string(),
            public_key: Some(public_key_str.trim().to_string()),
            passphrase: None,
        },
        &master_key,
    )
    .await
    .expect("store_secret must succeed");

    // Set auto_sign=true so the key appears in identities.
    upsert_ssh_settings(&app.db, 1, "ssh-keys", "id-test", true, false)
        .await
        .expect("upsert_ssh_settings must succeed");

    // GET /phylax/ssh/identities with master auth.
    let (status, body) = app
        .request_master("GET", "/phylax/ssh/identities", None)
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "identities endpoint must return 200; body={body}"
    );

    let identities = body["identities"]
        .as_array()
        .expect("response must have identities array");
    // Find the seeded key in the list.
    let entry = identities
        .iter()
        .find(|e| e["category"] == "ssh-keys" && e["name"] == "id-test")
        .expect("seeded key must appear in identities");

    // Public key must be populated and be a non-empty string.
    let public_openssh = entry["public_openssh"]
        .as_str()
        .expect("public_openssh must be a string");
    assert!(
        !public_openssh.is_empty(),
        "public_openssh must not be empty"
    );

    // auto_sign must be reflected as true.
    assert_eq!(
        entry["auto_sign"], true,
        "auto_sign must be true for the seeded key"
    );

    // The raw JSON body must not contain any private key material.
    let raw_json = body.to_string();
    assert!(
        !raw_json.contains("PRIVATE KEY"),
        "response must not contain private key material (found 'PRIVATE KEY')"
    );
    assert!(
        !raw_json.contains("BEGIN OPENSSH"),
        "response must not contain private key PEM envelope (found 'BEGIN OPENSSH')"
    );
}

/// Stub for the approval-gate path (auto_sign=false). This test is intentionally
/// ignored because it would require a 25-second wall-clock wait for the approval
/// timeout to fire. The gate's logic is unit-tested in the sign handler; the
/// manual e2e path is covered by Task 12.
#[tokio::test]
#[ignore = "approval-gate path takes up to 25 s; verified by gate unit logic and Task 12 e2e"]
async fn test_ssh_sign_auto_sign_false_requires_approval_gate() {
    // When auto_sign=false, POST /phylax/ssh/{category}/{name}/sign must
    // create a pending approval and block until approved or timed-out (25 s max).
    // This long-running path is not exercised in automated CI; see Task 12.
}
