//! Applies [`crate::rate_limit::PeerRateLimiter`] to `web::public_routes`
//! (section 6). See `rate_limit`'s own module doc comment for where the
//! caller IP this keys on actually comes from (`X-Forwarded-For`, set by
//! `gitquarry-core`'s proxy — never this process's own `ConnectInfo`,
//! which would just see core's loopback address).

use crate::access;
use crate::state::FederationState;
use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

const CLIENT_IP_HEADER: &str = "x-forwarded-for";

fn bearer_token(req: &Request) -> Option<String> {
    let value = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    Some(value.strip_prefix("Bearer ")?.trim().to_string())
}

fn client_ip(req: &Request) -> String {
    req.headers()
        .get(CLIENT_IP_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

/// Extracts the host portion of a peer's stored base URL — no general URL
/// parser (this workspace has none), just enough to strip a scheme and a
/// trailing `:port`/path for the plain string comparison
/// [`peer_override_for_ip`] does against a caller's IP.
fn url_host(url: &str) -> &str {
    let rest = url.split("://").nth(1).unwrap_or(url);
    rest.split(['/', ':']).next().unwrap_or(rest)
}

/// `Some(max)` if `ip` textually matches a *linked* peer's own
/// `web_base_url`/`git_base_url` host and that peer has a
/// `rate_limit_override` set — see `entities::Peer::rate_limit_override`'s
/// own doc comment on why this is a heuristic, not a guarantee.
async fn peer_override_for_ip(state: &FederationState, ip: &str) -> Option<u32> {
    let peers = access::list_all_peers(state.backend.as_ref(), &state.cipher)
        .await
        .ok()?;
    peers
        .into_iter()
        .find(|p| url_host(&p.web_base_url) == ip || url_host(&p.git_base_url) == ip)
        .and_then(|p| p.rate_limit_override)
}

/// Takes the already-extracted bearer/IP strings, never the `Request`
/// itself — holding a `&Request` across the `await`s below would make the
/// enclosing future non-`Send` (its body type isn't `Sync`), which
/// `axum::middleware::from_fn`'s boxed future requires.
async fn check(state: &FederationState, bearer: Option<String>, ip: String) -> bool {
    if let Some(secret) = bearer
        && let Ok(Some((owner, _token))) =
            access::resolve_by_token(state.backend.as_ref(), &state.cipher, &secret).await
    {
        return state
            .rate_limiter
            .check_token(&owner, state.rate_limit_token_max, state.rate_limit_token_window);
    }
    // No bearer token, or one that didn't resolve — the public bucket,
    // keyed by IP either way: a garbage `Authorization` header must not
    // let a caller dodge the public ceiling entirely.
    let max = if state.rate_limit_peer_override_by_ip {
        peer_override_for_ip(state, &ip)
            .await
            .unwrap_or(state.rate_limit_public_max)
    } else {
        state.rate_limit_public_max
    };
    state
        .rate_limiter
        .check_public(&ip, max, state.rate_limit_public_window)
}

type BoxResponseFuture = Pin<Box<dyn Future<Output = Response> + Send>>;

/// Returns the closure `axum::middleware::from_fn` wants, with `state`
/// captured — same pattern `web::require_internal_token`'s own caller
/// uses for its captured secret.
pub(super) fn layer(state: Arc<FederationState>) -> impl Fn(Request, Next) -> BoxResponseFuture + Clone {
    move |req: Request, next: Next| {
        let state = Arc::clone(&state);
        let bearer = bearer_token(&req);
        let ip = client_ip(&req);
        Box::pin(async move {
            if !check(&state, bearer, ip).await {
                return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
            }
            next.run(req).await
        })
    }
}
