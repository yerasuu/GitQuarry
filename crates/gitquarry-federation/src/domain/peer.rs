//! Every mutation of a [`crate::entities::Peer`] goes through here —
//! transport-agnostic, `anyhow::Result`, no CSRF/session/audit concerns
//! (those stay in whichever web handler calls this).

use crate::access;
use crate::entities::{Peer, PeerAccessLevel};
use crate::instance_client::verify_instance;
use crate::sister::{self, BanAction, BanEventWire};
use crate::time::{format_unix_secs, now_secs};
use anyhow::{Result, bail};
use s3db::{Backend, Cas, Cipher};

/// Create-only: `Ok(None)` if a peer with this `name` already exists.
/// Validated before any write, including a live round-trip against
/// `<web_base_url>/_api/instance`, so a dead or non-gitquarry URL fails
/// here with a clear error instead of sitting silently in storage.
/// `access_level` is the caller's call: `web::peers`'s manual add passes
/// `Full`, `gossip::run_auto_link_tick` passes `BrowseOnly` — see
/// `entities::Peer::access_level`'s own doc comment on why they differ.
/// `auto_linked` records *how* this add happened, independent of
/// `access_level` (a moderator can raise/lower the latter afterward) —
/// `web::peers`'s manual add passes `false`, `gossip::run_auto_link_tick`
/// passes `true` (see `entities::Peer::auto_linked`'s own doc comment).
///
/// `Err` (not the ordinary `Ok(None)` "already exists" case) if `name`
/// belongs to a currently-*banned* peer — section 6: re-adding over a ban
/// needs an explicit [`unban`] first, never an instant silent re-add, so
/// the caller gets a specific, actionable message instead of a generic
/// conflict.
pub async fn add(
    backend: &dyn Backend,
    cipher: &Cipher,
    name: &str,
    web_base_url: &str,
    git_base_url: &str,
    access_level: PeerAccessLevel,
    auto_linked: bool,
) -> Result<Option<Cas>> {
    access::validate_peer_name(name)?;
    access::validate_peer_url(web_base_url)?;
    access::validate_peer_url(git_base_url)?;
    if let Some(existing) = access::get_peer(backend, cipher, name).await?
        && existing.banned
    {
        bail!(
            "'{name}' was previously banned ({}) — unban it first",
            existing.ban_reason.as_deref().unwrap_or("no reason recorded")
        );
    }
    verify_instance(web_base_url).await?;
    let peer = Peer {
        name: name.to_string(),
        web_base_url: web_base_url.to_string(),
        git_base_url: git_base_url.to_string(),
        linked_at: format_unix_secs(now_secs()),
        access_level,
        auto_linked,
        ..Default::default()
    };
    let cas = access::put_peer(backend, cipher, &peer, None).await?;
    Ok(match cas {
        Cas::Ok => Some(cas),
        Cas::Conflict => None,
    })
}

/// Moderator/admin action: raises or lowers an existing linked peer's
/// [`PeerAccessLevel`] — the manual override `entities::Peer::access_level`'s
/// own doc comment describes ("a moderator can raise it afterward"). A
/// no-op, not an error, if `name` isn't currently linked — same tolerance
/// [`delete`] already has for an unknown name.
pub async fn set_access_level(
    backend: &dyn Backend,
    cipher: &Cipher,
    name: &str,
    access_level: PeerAccessLevel,
) -> Result<()> {
    let (peer, etag) = Peer::get(backend, cipher, name).await?;
    let Some(mut peer) = peer else {
        return Ok(());
    };
    peer.access_level = access_level;
    peer.put(backend, cipher, etag.as_deref()).await?;
    Ok(())
}

/// Best-effort, a no-op if `name` doesn't exist — unlinking a peer never
/// touches core's own `_remote/<peer>/*` mirrors (federation layer 2);
/// those stay on disk as a disposable cache until core's own mirror GC
/// reclaims them.
pub async fn delete(backend: &dyn Backend, cipher: &Cipher, name: &str) {
    access::delete_peer_object(backend, cipher, name).await;
}

/// `Err` if `reason` is empty/whitespace-only — a ban's reason is always
/// public (`entities::Peer::ban_reason`'s own doc comment) and mandatory,
/// never an afterthought. `Ok(false)` if `name` isn't currently linked at
/// all — nothing to ban. Revokes browse/fork/PR-proxy immediately: every
/// enforcement site checks `banned` before `access_level`, never a second
/// code path that could forget to.
pub async fn ban(backend: &dyn Backend, cipher: &Cipher, name: &str, reason: &str, banned_by: &str) -> Result<bool> {
    let reason = reason.trim();
    if reason.is_empty() {
        bail!("a ban needs a reason");
    }
    let (peer, etag) = Peer::get(backend, cipher, name).await?;
    let Some(mut peer) = peer else {
        return Ok(false);
    };
    peer.banned = true;
    peer.ban_reason = Some(reason.to_string());
    peer.banned_at = Some(format_unix_secs(now_secs()));
    peer.banned_by = banned_by.to_string();
    peer.put(backend, cipher, etag.as_deref()).await?;
    // Never propagate a ban this instance only learned about *from* a
    // sister — that would echo it straight back, and `banned_by`/
    // `ban_reason` wouldn't be this instance's own decision to push.
    if peer.synced_from_sister.is_none() {
        sister::push_ban_event(
            backend,
            cipher,
            &BanEventWire {
                peer_name: peer.name.clone(),
                git_base_url: peer.git_base_url.clone(),
                action: BanAction::Ban,
                reason: peer.ban_reason.clone(),
                banned_by: peer.banned_by.clone(),
                banned_at: peer.banned_at.clone(),
            },
        )
        .await;
    }
    Ok(true)
}

/// Clears the ban but deliberately does **not** restore whatever
/// `access_level` the peer held before it — resets to `BrowseOnly`, the
/// same minimal grant an auto-link starts at, requiring a moderator to
/// consciously raise it again. Without this, unbanning would silently
/// hand back `Full`/`BrowseAndFork` with a single click, undermining the
/// ban. `Ok(false)` if `name` isn't linked or isn't currently banned.
pub async fn unban(backend: &dyn Backend, cipher: &Cipher, name: &str) -> Result<bool> {
    let (peer, etag) = Peer::get(backend, cipher, name).await?;
    let Some(mut peer) = peer else {
        return Ok(false);
    };
    if !peer.banned {
        return Ok(false);
    }
    let was_synced_from_sister = peer.synced_from_sister.is_some();
    peer.banned = false;
    peer.ban_reason = None;
    peer.banned_at = None;
    peer.banned_by = String::new();
    peer.access_level = PeerAccessLevel::BrowseOnly;
    peer.synced_from_sister = None;
    let git_base_url = peer.git_base_url.clone();
    let peer_name = peer.name.clone();
    peer.put(backend, cipher, etag.as_deref()).await?;
    if !was_synced_from_sister {
        sister::push_ban_event(
            backend,
            cipher,
            &BanEventWire {
                peer_name,
                git_base_url,
                action: BanAction::Unban,
                reason: None,
                banned_by: String::new(),
                banned_at: None,
            },
        )
        .await;
    }
    Ok(true)
}
