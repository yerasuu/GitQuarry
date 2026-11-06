//! Outbound calls to *another gitquarry instance's* public federation API
//! (`/_api/*`, reached through that instance's own core-side proxy) —
//! peer-to-peer, never to this process's own core. Used today by
//! [`crate::domain::peer::add`]'s link-time verification; the gossip layer
//! (`docs/federation-binary-split-concept.md` section 5) reuses this same
//! client for its own independent re-verification of a gossiped candidate.

use crate::gossip::{AnnounceResponse, GossipCandidateWire};
use crate::sister::{BanEventWire, SisterBanEntry};
use anyhow::{Context, Result, bail};
use gitquarry_wire::{InstanceInfo, RepoSummary};
use std::time::Duration;

/// A peer that doesn't answer within this long is treated as down for this
/// request — never lets one unreachable peer hang an admin's `/_peers` add
/// form, or a gossip tick, indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .context("building the federation HTTP client")
}

/// Round-trips against `<web_base_url>/_api/instance` — called before a
/// peer/gossip candidate is ever persisted, so a dead or non-gitquarry URL
/// fails at that point with a clear error instead of sitting silently in
/// storage until the first browse/refresh attempt.
pub async fn verify_instance(web_base_url: &str) -> Result<InstanceInfo> {
    let client = http_client()?;
    let url = format!("{}/_api/instance", web_base_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    resp.json::<InstanceInfo>()
        .await
        .with_context(|| format!("{url} didn't return a valid gitquarry instance response"))
}

/// `<web_base_url>/_api/repos` — every repo the peer currently exposes.
pub async fn fetch_repos(web_base_url: &str) -> Result<Vec<RepoSummary>> {
    let client = http_client()?;
    let url = format!("{}/_api/repos", web_base_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    resp.json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))
}

/// `POST <web_base_url>/_federation/gossip/announce` — one gossip round
/// trip against another instance: tells it about `self_info` plus a
/// sample of what we already know, gets back its own current table
/// (anti-entropy push-pull). Reached through *that* instance's own core
/// proxy, same as every other `/_api/*`/`/_federation/*` call — never
/// loopback, this is genuinely peer-to-peer over the public internet.
pub async fn announce(
    web_base_url: &str,
    self_info: &GossipCandidateWire,
    known_peers: &[GossipCandidateWire],
) -> Result<AnnounceResponse> {
    let client = http_client()?;
    let url = format!("{}/_federation/gossip/announce", web_base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "instance": self_info, "known_peers": known_peers }))
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    resp.json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))
}

/// `POST <web_base_url>/_api/pulls/comment` — federation layer 4's PR-proxy
/// write, using the viewer's own linked [`crate::access::PeerCredential`]
/// as the bearer secret. Peer-to-peer, same as every other call in this
/// module; the peer's own `/_api/pulls/comment` is what actually decides
/// whether `credential_secret` grants write access to `repo_name`.
pub async fn post_comment(
    web_base_url: &str,
    repo_name: &str,
    number: u64,
    body: &str,
    credential_secret: &str,
) -> Result<()> {
    let client = http_client()?;
    let url = format!("{}/_api/pulls/comment", web_base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(credential_secret)
        .json(&serde_json::json!({ "repo": repo_name, "number": number, "body": body }))
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        let detail = resp.text().await.unwrap_or_default();
        bail!("{url} returned an error: {detail}");
    }
    Ok(())
}

/// `POST <web_base_url>/_federation/sister/handshake` — the initiator's
/// side of section 7's mutual-consent handshake. `Ok(true)` means the
/// other side already had `sister_requested` set for us too, and the
/// shared `secret` is now confirmed on both sides.
pub async fn sister_handshake(web_base_url: &str, my_name: &str, secret: &str) -> Result<bool> {
    let client = http_client()?;
    let url = format!("{}/_federation/sister/handshake", web_base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "instance_name": my_name, "secret": secret }))
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        ok: bool,
    }
    let resp: Resp = resp
        .json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))?;
    Ok(resp.ok)
}

/// `POST <web_base_url>/_federation/sister/verify` — the receiving side's
/// dial-back during a handshake: asks the instance at the peer's
/// *registered* URL whether it really staged `secret` for `my_name`. A
/// handshake body names its sender but can't prove it; only the real peer
/// can answer `true` here.
pub async fn verify_sister_secret(web_base_url: &str, my_name: &str, secret: &str) -> Result<bool> {
    let client = http_client()?;
    let url = format!("{}/_federation/sister/verify", web_base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "instance_name": my_name, "secret": secret }))
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        ok: bool,
    }
    let resp: Resp = resp
        .json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))?;
    Ok(resp.ok)
}

/// `POST <web_base_url>/_federation/sister/ban-event` — pushed immediately
/// on every local ban/unban to each currently-confirmed sister
/// (`sister::push_ban_event`).
pub async fn send_sister_ban_event(web_base_url: &str, secret: &str, event: &BanEventWire) -> Result<()> {
    let client = http_client()?;
    let url = format!("{}/_federation/sister/ban-event", web_base_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .bearer_auth(secret)
        .json(event)
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    Ok(())
}

/// `GET <web_base_url>/_federation/sister/bans` — the periodic
/// reconciliation pull (`sister::reconcile_with_sister`), full current ban
/// list, not just what changed since last time.
pub async fn fetch_sister_bans(web_base_url: &str, secret: &str) -> Result<Vec<SisterBanEntry>> {
    let client = http_client()?;
    let url = format!("{}/_federation/sister/bans", web_base_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .bearer_auth(secret)
        .send()
        .await
        .with_context(|| format!("couldn't reach {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned {}", resp.status());
    }
    resp.json()
        .await
        .with_context(|| format!("{url} returned malformed JSON"))
}
