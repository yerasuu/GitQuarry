//! `/_federation/gossip/announce` and `/_federation/gossip/peers` —
//! section 5's join/bootstrap endpoints. Reached through the receiving
//! instance's own core proxy, same as `/_api/*`; genuinely peer-to-peer,
//! never loopback-only despite the `/_federation/*` prefix looking
//! internal.
//!
//! Never trusts a gossiped entry without independently re-verifying it
//! first (`instance_client::verify_instance`) — both the announcer itself
//! and everything in its `known_peers` — so a malicious or buggy peer
//! feeding fabricated candidates can't poison this instance's
//! [`crate::gossip::GossipTable`] with entries that don't actually answer.

use crate::gossip::{AnnounceRequest, AnnounceResponse, GossipCandidateWire};
use crate::state::State_;
use crate::validate::{validate_git_base_url, validate_web_base_url};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// Caps one announce's `known_peers` fan-out — an unauthenticated caller
/// controls this list's length, so without a bound it could make this
/// instance issue an unbounded number of concurrent outbound probes
/// (`verify_instance` calls) from a single request. Matches the outbound
/// `gossip.sample(32)` size elsewhere in this module, doubled for slack.
const MAX_ANNOUNCE_CANDIDATES: usize = 64;

fn json_response<T: Serialize>(status: StatusCode, body: &T) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_else(|_| b"{}".to_vec());
    (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response()
}

/// Re-verifies `candidate` for real before ever letting it into the table
/// — a `None` verification (unreachable, or not a real gitquarry instance)
/// means it's silently dropped, never stored as a hint to try again later
/// (the announcer, or another peer, will just re-offer it on a future
/// tick if it's real). Both URLs are validated first, exactly as an
/// admin-supplied peer's are — a gossiped
/// candidate is a claim from an unauthenticated caller and must clear the
/// same bar before this instance ever dials it.
async fn verify_and_report(
    table: &crate::gossip::GossipTable,
    candidate: &GossipCandidateWire,
    reporter: Option<&str>,
) {
    if validate_web_base_url(&candidate.web_base_url).is_err()
        || validate_git_base_url(&candidate.git_base_url).is_err()
    {
        return;
    }
    if crate::instance_client::verify_instance(&candidate.web_base_url)
        .await
        .is_ok()
    {
        table.report(
            &candidate.name,
            &candidate.web_base_url,
            &candidate.git_base_url,
            reporter,
        );
    }
}

pub(crate) async fn handler_announce(State(state): State_, body: Bytes) -> Response {
    let Ok(mut req) = serde_json::from_slice::<AnnounceRequest>(&body) else {
        return (StatusCode::BAD_REQUEST, "invalid JSON body").into_response();
    };
    req.known_peers.truncate(MAX_ANNOUNCE_CANDIDATES);

    verify_and_report(&state.gossip, &req.instance, None).await;

    let mut set = tokio::task::JoinSet::new();
    let reporter = req.instance.name.clone();
    for candidate in req.known_peers {
        let gossip = std::sync::Arc::clone(&state.gossip);
        let reporter = reporter.clone();
        set.spawn(async move {
            verify_and_report(&gossip, &candidate, Some(&reporter)).await;
        });
    }
    while set.join_next().await.is_some() {}

    json_response(
        StatusCode::OK,
        &AnnounceResponse {
            known_peers: state.gossip.snapshot(),
        },
    )
}

/// Pull-only variant for a tick that doesn't also want to push its own
/// info this round.
pub(crate) async fn handler_peers(State(state): State_) -> Response {
    json_response(
        StatusCode::OK,
        &AnnounceResponse {
            known_peers: state.gossip.snapshot(),
        },
    )
}
