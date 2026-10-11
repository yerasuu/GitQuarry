//! Sister-instance mutual handshake and ban-list sync
//! (`docs/federation-binary-split-concept.md` section 7) — two instances
//! converge to an identical ban list, in both directions, including
//! future bans/unbans from either side.
//!
//! **Elevation, not a new link type**: only a peer this instance already
//! linked *manually* (`entities::Peer::auto_linked == false`) is eligible
//! — sister status implies enough trust to accept bans from the other
//! side, a materially higher bar than browse/fork access, so it inherits
//! `access_level`'s own "no unilateral trust" caution.
//!
//! **Mutual consent, never unilateral**: both sides must independently set
//! `sister_requested` before `sister_confirmed` ever becomes `true` on
//! either. The handshake secret reuses [`crate::access::PeerCredential`]'s
//! exact storage shape (`.conf/peer-tokens/<uid>/<peer_name>.json`) rather
//! than inventing a second secret-exchange mechanism — keyed under
//! [`gitquarry_wire::Uid::default`] (the "no claim"/system pseudo-user
//! sentinel that type's own doc comment describes) since this credential
//! belongs to the *instance*, not any one of its users.

use crate::access;
use crate::entities::{Peer, PeerAccessLevel};
use crate::time::{format_unix_secs, now_secs};
use crate::validate::validate_git_base_url;
use anyhow::{Result, bail};
use gitquarry_wire::Uid;
use s3db::{Backend, Cipher};
use serde::{Deserialize, Serialize};

/// The pseudo-user a sister handshake secret is filed under —
/// `PeerCredential` storage is per-user by design, but this credential
/// belongs to the instance as a whole.
fn instance_uid() -> Uid {
    Uid::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BanAction {
    Ban,
    Unban,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BanEventWire {
    pub peer_name: String,
    pub git_base_url: String,
    pub action: BanAction,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub banned_by: String,
    #[serde(default)]
    pub banned_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SisterBanEntry {
    pub peer_name: String,
    pub git_base_url: String,
    pub ban_reason: String,
    pub banned_by: String,
    pub banned_at: Option<String>,
}

/// Admin/moderator action: `Err` if `name` isn't linked at all, or was
/// auto-linked via gossip. Only sets the *local* flag — the actual
/// handshake attempt (an outbound HTTP call) is the caller's job
/// (`web::peers`), same domain/web split every other action here follows.
pub async fn request(backend: &dyn Backend, cipher: &Cipher, name: &str) -> Result<()> {
    let (peer, etag) = Peer::get(backend, cipher, name).await?;
    let Some(mut peer) = peer else {
        bail!("no such linked peer: {name}");
    };
    if peer.auto_linked {
        bail!("'{name}' was auto-linked via gossip — sister status requires a manually-linked peer");
    }
    peer.sister_requested = true;
    peer.put(backend, cipher, etag.as_deref()).await?;
    Ok(())
}

/// Inbound `POST /_federation/sister/handshake`: `true` only if this side
/// *also* already has `sister_requested` set for `announcer_name` — a
/// one-sided request is stored (via [`request`]) but never confirms
/// anything by itself.
///
/// `instance_name` in the request body is an unauthenticated, self-chosen
/// claim — this call can't verify the caller actually controls
/// `announcer_name`'s registered `web_base_url` (that needs a dial-back
/// verification this protocol doesn't do yet). To at least bound the
/// damage a spoofed claim can do: once a secret is confirmed for
/// `announcer_name`, a later call can only replay that exact secret
/// (idempotent retry of a lost response), never overwrite it with a
/// different one, and `sister_requested` is cleared on success so the
/// one-shot request/confirm cycle can't be replayed again after the fact.
pub async fn receive_handshake(
    backend: &dyn Backend,
    cipher: &Cipher,
    announcer_name: &str,
    secret: &str,
) -> Result<bool> {
    let (peer, etag) = Peer::get(backend, cipher, announcer_name).await?;
    let Some(mut peer) = peer else {
        return Ok(false);
    };
    if peer.auto_linked {
        return Ok(false);
    }
    // Checked before the `sister_requested` gate below, deliberately:
    // confirming clears `sister_requested` (a few lines down), so once a
    // handshake is confirmed this is the *only* branch a later call can
    // ever reach — putting it after that gate would make the idempotent-
    // retry path this comment documents permanently unreachable.
    if peer.sister_confirmed {
        let current = access::get_peer_credential(backend, cipher, &instance_uid(), announcer_name).await?;
        return Ok(current.is_some_and(|c| c.secret == secret));
    }
    if !peer.sister_requested {
        return Ok(false);
    }
    peer.sister_confirmed = true;
    peer.sister_requested = false;
    peer.put(backend, cipher, etag.as_deref()).await?;
    access::set_peer_credential(
        backend,
        cipher,
        &instance_uid(),
        announcer_name,
        secret,
        "sister handshake",
    )
    .await?;
    Ok(true)
}

/// Called by the initiator once the handshake round-trip comes back `ok`
/// — confirms locally and stores the same secret both sides now share.
pub async fn confirm_after_handshake(backend: &dyn Backend, cipher: &Cipher, name: &str, secret: &str) -> Result<()> {
    let (peer, etag) = Peer::get(backend, cipher, name).await?;
    let Some(mut peer) = peer else {
        bail!("no such linked peer: {name}");
    };
    peer.sister_confirmed = true;
    peer.put(backend, cipher, etag.as_deref()).await?;
    access::set_peer_credential(backend, cipher, &instance_uid(), name, secret, "sister handshake").await
}

/// The secret this instance shares with `sister_name`, if any — what
/// authenticates every outbound sister call.
pub async fn credential_for(backend: &dyn Backend, cipher: &Cipher, sister_name: &str) -> Result<Option<String>> {
    Ok(
        access::get_peer_credential(backend, cipher, &instance_uid(), sister_name)
            .await?
            .map(|c| c.secret),
    )
}

/// Finds which currently-`sister_confirmed` peer `secret` belongs to —
/// O(sister count), expected to stay tiny. What authenticates an inbound
/// `/_federation/sister/ban-event` or `/_federation/sister/bans` call: the
/// caller never states its own name, only proves it holds the shared
/// secret.
pub async fn sister_by_secret(backend: &dyn Backend, cipher: &Cipher, secret: &str) -> Result<Option<Peer>> {
    for peer in access::list_all_peers(backend, cipher).await? {
        if !peer.sister_confirmed {
            continue;
        }
        if let Some(stored) = access::get_peer_credential(backend, cipher, &instance_uid(), &peer.name).await?
            && stored.secret == secret
        {
            return Ok(Some(peer));
        }
    }
    Ok(None)
}

/// Applies one ban/unban event attributed to `sister_name` — a ban for a
/// name not known locally lands as a minimal, `BrowseOnly` stub `Peer`
/// (section 7: "estar baneado ya no implica haber sido linkeado antes").
/// Provenance (`banned_by`/`ban_reason`) always reflects the *original*
/// actor, never `sister_name` itself; [`entities::Peer::synced_from_sister`]
/// is the only field that records where this instance learned about it.
///
/// An unban only lifts a ban that this same sister put here: a ban this
/// instance issued itself, or learned from another sister, is not
/// `sister_name`'s to clear. Like a local [`crate::domain::peer::unban`],
/// it drops `access_level` to `BrowseOnly` instead of restoring it.
///
/// # Errors
///
/// `event.peer_name` and `event.git_base_url` come from another instance
/// and end up as an S3 key segment, a filesystem path (core's
/// `_remote/<peer>`) and a `git fetch` URL, so an invalid one is refused
/// before anything is stored.
pub async fn apply_ban_event(
    backend: &dyn Backend,
    cipher: &Cipher,
    sister_name: &str,
    event: &BanEventWire,
) -> Result<()> {
    access::validate_peer_name(&event.peer_name)?;
    let (existing, etag) = Peer::get(backend, cipher, &event.peer_name).await?;
    let peer = match (event.action, existing) {
        (BanAction::Ban, existing) => {
            validate_git_base_url(&event.git_base_url)?;
            let mut peer = existing.unwrap_or_else(|| Peer {
                name: event.peer_name.clone(),
                git_base_url: event.git_base_url.clone(),
                access_level: PeerAccessLevel::BrowseOnly,
                ..Default::default()
            });
            peer.banned = true;
            peer.ban_reason = event.reason.clone();
            peer.banned_at = event.banned_at.clone().or_else(|| Some(format_unix_secs(now_secs())));
            peer.banned_by = event.banned_by.clone();
            peer.synced_from_sister = Some(sister_name.to_string());
            peer
        }
        (BanAction::Unban, Some(mut peer)) => {
            if !peer.banned || peer.synced_from_sister.as_deref() != Some(sister_name) {
                return Ok(());
            }
            peer.banned = false;
            peer.ban_reason = None;
            peer.banned_at = None;
            peer.banned_by = String::new();
            peer.access_level = PeerAccessLevel::BrowseOnly;
            peer.synced_from_sister = None;
            peer
        }
        (BanAction::Unban, None) => return Ok(()),
    };
    // Best-effort CAS: a lost race here just means the next reconciliation
    // tick catches it too, same tolerance every other periodic sync in
    // this codebase already has.
    let _ = peer.put(backend, cipher, etag.as_deref()).await?;
    Ok(())
}

/// Pushes one local ban/unban to every currently-confirmed sister —
/// called right after `domain::peer::ban`/`unban` succeeds. Best-effort
/// per sister: one unreachable sister must never block the others or the
/// action that triggered this.
pub async fn push_ban_event(backend: &dyn Backend, cipher: &Cipher, event: &BanEventWire) {
    let sisters = match access::list_all_peers(backend, cipher).await {
        Ok(peers) => peers.into_iter().filter(|p| p.sister_confirmed).collect::<Vec<_>>(),
        Err(_) => return,
    };
    for sister in sisters {
        let Ok(Some(secret)) = credential_for(backend, cipher, &sister.name).await else {
            continue;
        };
        if let Err(e) = crate::instance_client::send_sister_ban_event(&sister.web_base_url, &secret, event).await {
            tracing::warn!(
                target: "gitquarry_federation::sister",
                sister = %sister.name,
                error = %format!("{e:#}"),
                "sister ban-event push failed"
            );
        }
    }
}

/// One full reconciliation round against `sister` (section 7 step 2):
/// pulls its complete current ban list and merges bans in, then mirrors
/// any local ban attributed to `sister` that it no longer lists (i.e. it
/// unbanned since the last sync) back to unbanned. Doesn't push local-
/// origin bans outward — [`push_ban_event`] already does that at the
/// moment of the action; this only needs to catch what a lost push missed
/// on the *pull* side, converging both directions over enough ticks.
pub async fn reconcile_with_sister(backend: &dyn Backend, cipher: &Cipher, sister: &Peer, secret: &str) -> Result<()> {
    let remote_bans = crate::instance_client::fetch_sister_bans(&sister.web_base_url, secret).await?;
    let remote_names: std::collections::HashSet<&str> = remote_bans.iter().map(|b| b.peer_name.as_str()).collect();

    for entry in &remote_bans {
        let event = BanEventWire {
            peer_name: entry.peer_name.clone(),
            git_base_url: entry.git_base_url.clone(),
            action: BanAction::Ban,
            reason: Some(entry.ban_reason.clone()),
            banned_by: entry.banned_by.clone(),
            banned_at: entry.banned_at.clone(),
        };
        // One entry the sister got wrong (an invalid name or URL) must not
        // hold back every other ban in the same list.
        if let Err(e) = apply_ban_event(backend, cipher, &sister.name, &event).await {
            tracing::warn!(
                target: "gitquarry_federation::sister",
                sister = %sister.name,
                peer = %entry.peer_name,
                error = %format!("{e:#}"),
                "skipped a sister ban entry"
            );
        }
    }

    let locally_synced_from_this_sister = access::list_all_peers(backend, cipher)
        .await?
        .into_iter()
        .filter(|p| p.banned && p.synced_from_sister.as_deref() == Some(sister.name.as_str()));
    for peer in locally_synced_from_this_sister {
        if !remote_names.contains(peer.name.as_str()) {
            apply_ban_event(
                backend,
                cipher,
                &sister.name,
                &BanEventWire {
                    peer_name: peer.name.clone(),
                    git_base_url: peer.git_base_url.clone(),
                    action: BanAction::Unban,
                    reason: None,
                    banned_by: String::new(),
                    banned_at: None,
                },
            )
            .await?;
        }
    }
    Ok(())
}

/// Spawned once from `main.rs`, runs forever: every `interval_secs`,
/// reconciles against each currently `sister_confirmed` peer. One
/// unreachable/failed sister is logged and skipped, never aborts the tick
/// or blocks the others — same tolerance `gossip::run_periodic_tick`
/// already has for one dead seed peer.
pub async fn run_periodic_reconcile_tick(
    backend: std::sync::Arc<dyn Backend>,
    cipher: std::sync::Arc<Cipher>,
    interval_secs: u64,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs.max(1)));
    loop {
        interval.tick().await;
        let sisters = match access::list_all_peers(backend.as_ref(), &cipher).await {
            Ok(peers) => peers.into_iter().filter(|p| p.sister_confirmed).collect::<Vec<_>>(),
            Err(e) => {
                tracing::warn!(target: "gitquarry_federation::sister", error = %format!("{e:#}"), "sister reconcile tick couldn't list peers");
                continue;
            }
        };
        for sister in sisters {
            let secret = match credential_for(backend.as_ref(), &cipher, &sister.name).await {
                Ok(Some(secret)) => secret,
                _ => continue,
            };
            if let Err(e) = reconcile_with_sister(backend.as_ref(), &cipher, &sister, &secret).await {
                tracing::warn!(
                    target: "gitquarry_federation::sister",
                    sister = %sister.name,
                    error = %format!("{e:#}"),
                    "sister reconciliation failed"
                );
            }
        }
    }
}
