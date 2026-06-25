//! Integration tests for user deactivation credential revocation.
//!
//! Deactivating a user must (a) cascade-revoke every credential the user
//! holds (api_keys, identity_keys, identities, mcp_tokens) and (b) be
//! enforced at the credential validation chokepoint even for rows that
//! somehow remain active, via the users.is_active subquery.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{bootstrap_admin_key, get, send, test_app};
use kleos_lib::auth::Scope;
use kleos_lib::auth_piv::RequestSigner;
use serde_json::json;

/// Create a secondary (non-owner) user via the admin API and return its id.
async fn create_user(app: &axum::Router, admin_key: &str, username: &str) -> i64 {
    let request = Request::builder()
        .method("POST")
        .uri("/users")
        .header("Authorization", format!("Bearer {}", admin_key))
        .header("Content-Type", "application/json")
        .body(Body::from(json!({ "username": username }).to_string()))
        .unwrap();
    let (status, body) = send(app, request).await;
    assert_eq!(status, StatusCode::CREATED, "create_user failed: {body}");
    body["id"].as_i64().expect("created user id")
}

/// Build a signed GET request from the signer for the given path.
fn signed_get(signer: &RequestSigner, path: &str) -> Request<Body> {
    let signed = signer
        .sign_request("GET", path, "", b"")
        .expect("sign request");
    Request::builder()
        .method("GET")
        .uri(path)
        .header("X-Kleos-Sig", &signed.sig_hex)
        .header("X-Kleos-Algo", signed.algo.as_str())
        .header("X-Kleos-Identity", &signed.identity_hash)
        .header("X-Kleos-Ts", signed.ts_ms.to_string())
        .header("X-Kleos-Nonce", &signed.nonce)
        .header("X-Kleos-Key-Fp", &signed.key_fp)
        .header("X-Kleos-Host", &signed.host_label)
        .header("X-Kleos-Agent", &signed.agent_label)
        .header("X-Kleos-Model", &signed.model_label)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn deactivation_revokes_all_credentials() {
    let (app, state) = test_app().await;
    let admin_key = bootstrap_admin_key(&app).await;
    let uid = create_user(&app, &admin_key, "victim").await;

    // Bearer credential for the victim user.
    let (_api_key, raw_key) = kleos_lib::auth::create_key(
        &state.db,
        uid,
        "victim-key",
        vec![Scope::Read, Scope::Write],
        None,
    )
    .await
    .expect("create victim api key");

    // Identity-key credential for the victim user (soft Ed25519). The
    // identities row is auto-registered on the first signed request.
    let signer =
        RequestSigner::from_key_bytes([7u8; 32], "victim-host", "victim-agent", "victim-model");
    let pem = signer.pubkey_pem().to_string();
    let fp = signer.fingerprint().to_string();
    let ik_id = state
        .db
        .write(move |conn| {
            Ok(conn.query_row(
                "INSERT INTO identity_keys (user_id, tier, algo, pubkey_pem, pubkey_fingerprint, host_label, scopes)
                 VALUES (?1, 'soft', 'ed25519', ?2, ?3, 'victim-host', 'read,write') RETURNING id",
                rusqlite::params![uid, pem, fp],
                |row| row.get::<_, i64>(0),
            )?)
        })
        .await
        .expect("insert identity key");

    // MCP token credential for the victim user.
    state
        .db
        .write(move |conn| {
            conn.execute(
                "INSERT INTO mcp_tokens (jti, user_id, identity_key_id, kid, scopes, expires_at)
                 VALUES ('victim-jti', ?1, ?2, 'victim-kid', 'read', datetime('now', '+1 hour'))",
                rusqlite::params![uid, ik_id],
            )?;
            Ok(())
        })
        .await
        .expect("insert mcp token");

    // Both credentials work while the user is active.
    let (status, _) = get(&app, "/projects", &raw_key).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "bearer auth must work pre-deactivation"
    );
    let (status, body) = send(&app, signed_get(&signer, "/projects")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "signed auth must work pre-deactivation: {body}"
    );

    // Chokepoint isolation: flip users.is_active directly (no cascade) and
    // verify validation fails for rows that are still active themselves.
    let uid_off = uid;
    state
        .db
        .write(move |conn| {
            conn.execute(
                "UPDATE users SET is_active = 0 WHERE id = ?1",
                rusqlite::params![uid_off],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let (status, _) = get(&app, "/projects", &raw_key).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "bearer chokepoint must consult users.is_active"
    );
    let (status, _) = send(&app, signed_get(&signer, "/projects")).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "signed chokepoint must consult users.is_active"
    );

    // Reactivate and confirm the chokepoint is non-destructive.
    let uid_on = uid;
    state
        .db
        .write(move |conn| {
            conn.execute(
                "UPDATE users SET is_active = 1 WHERE id = ?1",
                rusqlite::params![uid_on],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let (status, _) = get(&app, "/projects", &raw_key).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "bearer auth must recover after reactivation"
    );

    // Deactivate through the admin endpoint: the cascade must revoke
    // every credential row in the same transaction.
    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/users/{uid}"))
        .header("Authorization", format!("Bearer {}", admin_key))
        .body(Body::empty())
        .unwrap();
    let (status, body) = send(&app, request).await;
    assert_eq!(status, StatusCode::OK, "deactivate failed: {body}");
    assert_eq!(body["deactivated"], json!(true));

    let (status, _) = get(&app, "/projects", &raw_key).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "bearer auth must fail after deactivation"
    );
    let (status, _) = send(&app, signed_get(&signer, "/projects")).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "signed auth must fail after deactivation"
    );

    // Every credential row belonging to the user is now revoked.
    let uid_check = uid;
    let (api_active, ik_active, ik_revoked_at_set, identities_active, mcp_active) = state
        .db
        .read(move |conn| {
            let api: i64 = conn.query_row(
                "SELECT COUNT(*) FROM api_keys WHERE user_id = ?1 AND is_active = 1",
                rusqlite::params![uid_check],
                |row| row.get(0),
            )?;
            let ik: i64 = conn.query_row(
                "SELECT COUNT(*) FROM identity_keys WHERE user_id = ?1 AND is_active = 1",
                rusqlite::params![uid_check],
                |row| row.get(0),
            )?;
            let ik_revoked: i64 = conn.query_row(
                "SELECT COUNT(*) FROM identity_keys WHERE user_id = ?1 AND revoked_at IS NOT NULL",
                rusqlite::params![uid_check],
                |row| row.get(0),
            )?;
            let idents: i64 = conn.query_row(
                "SELECT COUNT(*) FROM identities WHERE is_active = 1 AND identity_key_id IN
                 (SELECT id FROM identity_keys WHERE user_id = ?1)",
                rusqlite::params![uid_check],
                |row| row.get(0),
            )?;
            let mcp: i64 = conn.query_row(
                "SELECT COUNT(*) FROM mcp_tokens WHERE user_id = ?1 AND is_active = 1",
                rusqlite::params![uid_check],
                |row| row.get(0),
            )?;
            Ok((api, ik, ik_revoked, idents, mcp))
        })
        .await
        .unwrap();
    assert_eq!(api_active, 0, "api_keys must be revoked");
    assert_eq!(ik_active, 0, "identity_keys must be revoked");
    assert!(
        ik_revoked_at_set >= 1,
        "identity_keys must record revoked_at"
    );
    assert_eq!(
        identities_active, 0,
        "identities must be revoked via identity_keys join"
    );
    assert_eq!(mcp_active, 0, "mcp_tokens must be revoked");

    // The owner account stays protected.
    let request = Request::builder()
        .method("DELETE")
        .uri("/users/1")
        .header("Authorization", format!("Bearer {}", admin_key))
        .body(Body::empty())
        .unwrap();
    let (status, _) = send(&app, request).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "owner must not be deactivatable"
    );
}
