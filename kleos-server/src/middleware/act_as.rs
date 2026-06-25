//! Act-as delegation middleware: the single authorization chokepoint for
//! Space Sharing's whole-instance access.
//!
//! A request may carry `X-Kleos-Act-As: <owner_user_id>` to ask to operate
//! inside another user's shard. This middleware runs after auth (so the
//! `AuthContext` exists) and before any handler or the `ResolvedDb` extractor.
//! It authorizes the delegation ONCE here:
//!
//! - caller acting as themselves      -> no-op (no header, or header == self)
//! - caller holds Admin               -> god-mode, always allowed
//! - an `instance_grants` row covers the request's access need -> allowed
//! - otherwise                        -> 403, request never reaches the handler
//!
//! On success it sets `AuthContext::act_as` so tenant DATA operations follow
//! the delegation via `effective_user_id()`, while `user_id` stays the real
//! caller for control-plane authorization and audit. Authorizing here, at one
//! place, is why no individual query site has to re-check access: a request
//! that is not authorized here never reaches another tenant's data.

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use kleos_lib::auth::{AuthContext, Scope};
use kleos_lib::spaces::{self, InstanceAccess};
use serde_json::json;

use crate::state::AppState;

/// HTTP header naming the act-as target owner for delegated shard access.
pub const ACT_AS_HEADER: &str = "x-kleos-act-as";

/// Map an HTTP method to the minimum grant level it requires: a read for safe
/// methods, a write for mutating ones. This collapses SD1 (read isolation) and
/// SD3 (write gating) into one decision at the chokepoint.
fn min_access_for_method(method: &Method) -> InstanceAccess {
    match *method {
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE => InstanceAccess::Read,
        _ => InstanceAccess::Write,
    }
}

/// Build a JSON error response with the given status.
fn deny(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[tracing::instrument(skip_all, fields(middleware = "server.act_as"))]
pub async fn act_as_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    // No act-as header: the overwhelmingly common path. Leave the request
    // untouched so the caller operates inside their own shard.
    let Some(raw) = request
        .headers()
        .get(ACT_AS_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
    else {
        return next.run(request).await;
    };

    // Delegation requires an authenticated principal to delegate FROM.
    let Some(auth) = request.extensions().get::<AuthContext>().cloned() else {
        return deny(
            StatusCode::UNAUTHORIZED,
            "act-as requires an authenticated request",
        );
    };

    let target_owner: i64 = match raw.parse() {
        Ok(id) => id,
        Err(_) => {
            return deny(
                StatusCode::BAD_REQUEST,
                "invalid X-Kleos-Act-As header: expected an integer user id",
            );
        }
    };

    // Acting as yourself is a no-op: leave act_as unset and resolve your own
    // shard, regardless of any grant.
    if target_owner == auth.user_id {
        return next.run(request).await;
    }

    // Authorize the delegation. Admin is god-mode (SD4) and short-circuits the
    // grant lookup; otherwise a grant must cover the method's access need.
    if !auth.has_scope(&Scope::Admin) {
        let min_access = min_access_for_method(request.method());
        let granted =
            match spaces::lookup_instance_grant(&state.db, target_owner, auth.user_id).await {
                Ok(g) => g,
                Err(e) => {
                    return deny(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("grant lookup failed: {e}"),
                    );
                }
            };
        let authorized = granted.map(|a| a.satisfies(min_access)).unwrap_or(false);
        if !authorized {
            return deny(
                StatusCode::FORBIDDEN,
                "no grant authorizes acting as the requested owner",
            );
        }
    }

    // SD5: record the delegated resolution for forensic accountability.
    // Fire-and-forget so the request is not gated on the audit write; the real
    // caller stays the actor, the target owner is the resource.
    {
        let db = state.db.clone();
        let actor = auth.user_id;
        let owner = target_owner;
        let access = min_access_for_method(request.method()).as_str();
        tokio::spawn(async move {
            let _ = kleos_lib::audit::log_mutation(
                &db,
                "instance.act_as",
                "shard",
                &owner.to_string(),
                Some(&actor.to_string()),
                Some(actor),
                None,
                Some(json!({ "owner": owner, "actor": actor, "access": access })),
            )
            .await;
        });
    }

    // Authorized: stamp the delegation onto the AuthContext so tenant DATA
    // operations follow it via effective_user_id(). user_id stays the caller.
    let mut delegated = auth;
    delegated.act_as = Some(target_owner);
    request.extensions_mut().insert(delegated);

    next.run(request).await
}
