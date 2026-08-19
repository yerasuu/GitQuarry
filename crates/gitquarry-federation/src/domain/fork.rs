//! Federation layer 2's fork request — the thin half that stayed in
//! federation after the git/storage mechanic moved to
//! `gitquarry-core::domain::remote_fork::seed_from_remote`
//! (`docs/federation-binary-split-concept.md` section 2). All this does is
//! validate the link exists and forward to core's own
//! `POST /_internal/repos/fork-from-remote`, which re-verifies the
//! destination ownership itself server-side — this module trusts nothing
//! it forwards, but doesn't need to duplicate that check either.

use crate::access;
use crate::core_client::{CoreClient, ForkFromRemote, ForkOutcome};
use crate::entities::PeerAccessLevel;
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::{Backend, Cipher};

pub enum DestOwnerKind {
    User,
    Group,
}

impl DestOwnerKind {
    fn as_str(&self) -> &'static str {
        match self {
            DestOwnerKind::User => "user",
            DestOwnerKind::Group => "group",
        }
    }
}

pub struct RequestFork<'a> {
    pub peer_name: &'a str,
    pub source_repo_name: &'a str,
    pub dest_full_name: &'a str,
    pub dest_owner_kind: DestOwnerKind,
    pub dest_owner_uid: &'a Uid,
    pub description: Option<String>,
    pub acting_username: &'a str,
}

/// `Err` if `req.peer_name` isn't a peer this instance actually has
/// linked, is currently banned, or is linked at a `BrowseOnly`
/// [`PeerAccessLevel`] (section 6: forking needs at least
/// `BrowseAndFork`) — `banned` is checked *before* `access_level`, so a
/// banned peer is rejected no matter what tier it was linked at (same
/// "one code path, never a bypassable second one" discipline
/// `entities::Peer::banned`'s own doc comment describes).
pub async fn request_fork(
    backend: &dyn Backend,
    cipher: &Cipher,
    core: &CoreClient,
    req: RequestFork<'_>,
) -> Result<ForkOutcome> {
    let Some(peer) = access::get_peer(backend, cipher, req.peer_name).await? else {
        bail!("no such linked peer: {}", req.peer_name);
    };
    if peer.banned {
        bail!(
            "peer '{}' is banned: {}",
            req.peer_name,
            peer.ban_reason.as_deref().unwrap_or("no reason recorded")
        );
    }
    if peer.access_level < PeerAccessLevel::BrowseAndFork {
        bail!(
            "peer '{}' isn't allowed to be forked from (access level too low)",
            req.peer_name
        );
    }
    core.fork_from_remote(&ForkFromRemote {
        source_git_base_url: &peer.git_base_url,
        source_repo_name: req.source_repo_name,
        dest_full_name: req.dest_full_name,
        dest_owner_kind: req.dest_owner_kind.as_str(),
        dest_owner_uid: req.dest_owner_uid,
        description: req.description,
        peer_label: &peer.name,
        acting_username: req.acting_username,
    })
    .await
}
