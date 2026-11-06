//! Outbound *informational* lookups core makes into `gitquarry-federation`'s
//! loopback API (`docs/federation-binary-split-concept.md` section 3A) —
//! distinct from [`super::federation_proxy`] (which forwards a whole
//! inbound axum request/response byte-for-byte): these return typed data
//! for core's own templates (the `/` index's federated section,
//! `/_search`'s peer-metadata hits), never anything shown straight back to
//! a browser unmodified.

use gitquarry_core::app::FederationLink;
use gitquarry_wire::ipc::SignedRequestBuilderExt;
use gitquarry_wire::{RepoSummary, Uid};
use serde::{Deserialize, Serialize};

const USER_UID_HEADER: &str = "x-gitquarry-user-uid";

#[derive(Deserialize)]
struct PeerRepos {
    peer_name: String,
    repos: Vec<RepoSummary>,
}

/// A linked peer, as federation reports it — deliberately not the full
/// `gitquarry-federation::entities::Peer` (core doesn't depend on that
/// crate at all, by design; see the workspace split doc). Just what
/// `webui::remote_repo`'s browse view needs.
#[derive(Deserialize)]
pub(crate) struct PeerLookup {
    pub web_base_url: String,
    pub git_base_url: String,
    /// Checked *before* anything else at every browse/fork call site —
    /// same "one code path, never a bypassable second one" discipline
    /// `gitquarry-federation::entities::Peer::banned`'s own doc comment
    /// describes; a banned peer is cut off from browsing here too, not
    /// just fork/PR-proxy.
    pub banned: bool,
}

/// `GET /_internal-out/peer/{name}` — feeds `/_remote/{peer}/{*repo}`'s
/// peer resolution, replacing a local `.conf/peers` read now that `Peer`
/// is federation-owned storage. `None` for an unknown peer *or* an
/// unreachable federation process — both mean "nothing to browse here",
/// same as the old local lookup returning `None` for an unknown name.
pub(crate) async fn lookup_peer(federation: &FederationLink, name: &str) -> Option<PeerLookup> {
    let url = format!(
        "{}/_internal-out/peer/{}",
        federation.base_url.trim_end_matches('/'),
        crate::webui::url_encode(name)
    );
    let resp = federation
        .http
        .get(&url)
        .signed(&federation.internal_token)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<PeerLookup>().await.ok()
}

#[derive(Deserialize)]
struct CredentialLookup {
    secret: String,
}

/// `GET /_internal-out/credential/{peer_name}` — the viewer's own linked
/// `PeerCredential` secret for `peer_name`, so core can call that peer's
/// public `/_api/pulls*` directly with it (the PR-proxy tab's read side
/// stays entirely core-side — see `gitquarry-federation::web::remote_pulls`'s
/// own doc comment on why only the *write* needs federation). `None` if
/// nothing's linked, or federation is unreachable.
pub(crate) async fn lookup_credential(federation: &FederationLink, peer_name: &str, user_uid: &Uid) -> Option<String> {
    let url = format!(
        "{}/_internal-out/credential/{}",
        federation.base_url.trim_end_matches('/'),
        crate::webui::url_encode(peer_name)
    );
    let resp = federation
        .http
        .get(&url)
        .signed(&federation.internal_token)
        .header(USER_UID_HEADER, user_uid.to_string())
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<CredentialLookup>().await.ok().map(|c| c.secret)
}

#[derive(Serialize)]
pub(crate) struct ForkRequest<'a> {
    pub peer_name: &'a str,
    pub source_repo_name: &'a str,
    pub dest_full_name: &'a str,
    /// `"user"` or `"group"` — matches `gitquarry_federation::domain::fork::DestOwnerKind`'s
    /// own wire repr on the other side of this call.
    pub dest_owner_kind: &'a str,
    pub dest_owner_uid: &'a Uid,
    pub description: Option<String>,
    pub acting_username: &'a str,
}

#[derive(Deserialize)]
pub(crate) struct ForkOutcome {
    pub ok: bool,
    /// Federation's own response also carries `repo_uid` on success — not
    /// captured here, `webui::fork::handler_fork_post` already knows the
    /// destination's full name (it built it) and redirects there directly.
    pub reason: Option<String>,
}

/// `POST /_internal-out/fork` — `webui::fork::handler_fork_post`'s
/// peer-fork branch. Core already authenticated the session and resolved
/// the destination owner itself before calling this; federation only
/// re-validates the *source* peer link (not banned, at least
/// `BrowseAndFork`) before forwarding to core's own
/// `POST /_internal/repos/fork-from-remote`, which does the actual git
/// mechanic. A transport failure (federation unreachable) surfaces as
/// `ok: false` with a generic reason, same shape as a validation rejection.
pub(crate) async fn request_fork(federation: &FederationLink, req: &ForkRequest<'_>) -> ForkOutcome {
    let url = format!("{}/_internal-out/fork", federation.base_url.trim_end_matches('/'));
    let Ok(resp) = federation
        .http
        .post(&url)
        .signed(&federation.internal_token)
        .json(req)
        .send()
        .await
    else {
        return ForkOutcome {
            ok: false,
            reason: Some("couldn't reach federation".to_string()),
        };
    };
    resp.json::<ForkOutcome>().await.unwrap_or(ForkOutcome {
        ok: false,
        reason: Some("federation returned a malformed response".to_string()),
    })
}

/// `GET /_internal-out/peer-repos` — every linked, non-banned peer's
/// current repo list. Replaces `AppContext::peer_cache` wherever
/// `ctx.federation` is configured (section 8: the `/` index's federated
/// section and `/_search`'s peer-metadata hits both call this instead).
/// Best-effort: an unreachable federation process or a malformed reply
/// yields an empty list, same "never a hard error surfaced to a visitor"
/// tolerance `AppContext::peer_cache` itself already had for one down peer.
pub(crate) async fn peer_repos(federation: &FederationLink) -> Vec<(String, Vec<RepoSummary>)> {
    let url = format!("{}/_internal-out/peer-repos", federation.base_url.trim_end_matches('/'));
    let Ok(resp) = federation
        .http
        .get(&url)
        .signed(&federation.internal_token)
        .send()
        .await
    else {
        return Vec::new();
    };
    let Ok(entries) = resp.json::<Vec<PeerRepos>>().await else {
        return Vec::new();
    };
    entries.into_iter().map(|e| (e.peer_name, e.repos)).collect()
}
