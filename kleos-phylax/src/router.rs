//! Phylax route tree -- extends credd's base router with /phylax/* endpoints.

use axum::extract::DefaultBodyLimit;
use axum::middleware;
use axum::routing::{delete, get, post, put};
use axum::Router;
use std::time::Duration;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::handlers::{
    approvals, ecdh, kleos_token, leases, namespaces, policies, resolve_modes, ssh, ssh_ca,
    ssh_sign,
};
use crate::middleware::policy_check_middleware;
use crate::state::PhylaxState;
use kleos_credd::auth::{auth_middleware, preauth_rate_limit};
use kleos_credd::state::AppState;
use kleos_credd::{CREDD_BODY_LIMIT, CREDD_REQUEST_TIMEOUT_SECS};

/// Request window for the SSH CA sign/mint routes. These can block on an
/// out-of-band human approval (push to phone, a human taps), which takes longer
/// than the global default; bound it generously while still capping the wait.
const SSH_CA_REQUEST_TIMEOUT_SECS: u64 = 120;

/// Build the Phylax extension routes. These are merged into the credd
/// router by the phylaxd binary.
pub fn phylax_routes(state: AppState) -> Router<PhylaxState> {
    // The SSH CA routes get a longer request window than everything else, since
    // the M3 branch polls for a human approval. Scoped to just these two routes
    // so all other endpoints keep the snappy global timeout.
    let ssh_ca_routes = Router::new()
        .route("/phylax/ssh-ca/sign", post(ssh_ca::sign))
        .route("/phylax/ssh-ca/mint", post(ssh_ca::mint))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(SSH_CA_REQUEST_TIMEOUT_SECS),
        ));

    Router::new()
        // Approval workflows
        .route("/phylax/approvals", post(approvals::request_approval))
        .route("/phylax/approvals", get(approvals::list_approvals))
        .route("/phylax/approvals/{id}", get(approvals::get_approval))
        .route("/phylax/approvals/{id}", put(approvals::decide_approval))
        .route(
            "/phylax/approvals/{id}/wait",
            get(approvals::wait_for_decision),
        )
        // Capability-token approval decision. Auth-exempt in auth_middleware:
        // the single-use token presented in the body is the capability.
        .route(
            "/phylax/approvals/{id}/decide-token",
            post(approvals::decide_with_token),
        )
        // Leases
        .route("/phylax/leases", get(leases::list_leases))
        .route("/phylax/leases/{jti}/redeem", post(leases::redeem_lease))
        // Access policies
        .route("/phylax/policies", get(policies::list_policies))
        .route("/phylax/policies", post(policies::create_policy))
        .route("/phylax/policies/{id}", put(policies::update_policy))
        .route("/phylax/policies/{id}", delete(policies::delete_policy))
        // Namespaces
        .route("/phylax/namespaces", get(namespaces::list_namespaces))
        // Non-plaintext resolve modes: the secret is used server-side and
        // only the operation result returns. Mounted under /resolve/ so the
        // policy middleware intercepts them like the credd resolve routes.
        .route("/resolve/verify", post(resolve_modes::verify_payload))
        .route("/resolve/sign", post(resolve_modes::sign_payload))
        .route("/resolve/derive", post(resolve_modes::derive_key_material))
        .route("/resolve/exec", post(resolve_modes::exec_command))
        // Keyless Kleos token broker (Unix-socket + SO_PEERCRED gated; the
        // handler enforces both, and auth_middleware skips bearer auth for it).
        .route("/phylax/kleos/token", post(kleos_token::mint_kleos_token))
        // ECDH
        .route("/phylax/ecdh/challenge", post(ecdh::issue_challenge))
        .route("/phylax/ecdh/enroll", post(ecdh::enroll_pubkey))
        .route("/phylax/ecdh/revoke", post(ecdh::revoke_pubkey))
        // SSH key operations -- literal path before wildcard so it is not shadowed.
        .route("/phylax/ssh/identities", get(ssh_sign::identities))
        // SSH signing
        .route("/phylax/ssh/{category}/{name}/sign", post(ssh_sign::sign))
        // SSH key settings
        .route(
            "/phylax/ssh/{category}/{name}",
            get(ssh::get_settings).put(ssh::update_settings),
        )
        // Global request window for every route above.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(CREDD_REQUEST_TIMEOUT_SECS),
        ))
        // SSH certificate authority routes carry their own longer timeout.
        .merge(ssh_ca_routes)
        // Apply the same auth and preauth protections to all routes.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(middleware::from_fn_with_state(state, preauth_rate_limit))
        .layer(DefaultBodyLimit::max(CREDD_BODY_LIMIT))
        .layer(TraceLayer::new_for_http())
}

/// Compose credd base routes and phylax extensions into a single router.
///
/// The returned app keeps credd middleware semantics, then inserts
/// `policy_check_middleware` so /resolve/* requests can return approval
/// responses before credd plaintext handlers execute.
pub fn compose_router(state: AppState) -> Router {
    compose_router_with_phylax_state(PhylaxState::from_app_state(state))
}

/// Compose credd base routes and Phylax extensions with an explicit Phylax state.
pub fn compose_router_with_phylax_state(phylax_state: PhylaxState) -> Router {
    let state = phylax_state.inner.clone();
    // Merge credd base routes with phylax extension routes and enforce policy
    // interception on all resolve endpoints before the plaintext resolve handlers run.
    let app = kleos_credd::credd_routes::<PhylaxState>(state.clone())
        .merge(phylax_routes(state))
        .route_layer(middleware::from_fn_with_state(
            phylax_state.clone(),
            policy_check_middleware,
        ));

    app.with_state(phylax_state)
}
