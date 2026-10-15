//! Gossip P2P discovery (`docs/federation-binary-split-concept.md` section
//! 5) — replaces the single-binary predecessor's admin-linked-only,
//! pull-based `PeerCache`/`run_periodic_refresh` with peer-sampling gossip:
//! federation binaries exchange candidate lists directly, and a candidate
//! corroborated by enough already-linked sources for long enough
//! auto-promotes to a real [`crate::entities::Peer`].
//!
//! Two peer concepts, deliberately kept separate: [`crate::entities::Peer`]
//! (linked, browsable/forkable/searchable) vs. [`GossipTable`]'s
//! candidates (unlinked, unverified-until-independently-checked, just a
//! catalog). Nothing here ever makes a candidate browsable/forkable
//! directly — only [`crate::domain::peer::add`] (the same path a manual
//! admin-add uses) does that, called from the auto-link tick once a
//! candidate qualifies.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Oldest-by-`last_seen` entries get dropped first once over this —
/// candidates are cheap, unverified-until-promoted data, a generous cap is
/// just a memory bound, not a correctness concern.
const MAX_CANDIDATES: usize = 500;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// One instance's self-description, both in an outbound announce (who
/// *we* are) and inside `known_peers` (who *they've* seen).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipCandidateWire {
    pub name: String,
    pub web_base_url: String,
    pub git_base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct AnnounceRequest {
    pub instance: GossipCandidateWire,
    #[serde(default)]
    pub known_peers: Vec<GossipCandidateWire>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AnnounceResponse {
    pub known_peers: Vec<GossipCandidateWire>,
}

#[derive(Debug, Clone)]
struct Candidate {
    web_base_url: String,
    git_base_url: String,
    first_seen: u64,
    last_seen: u64,
    /// Names of peers that reported this candidate — restricted to
    /// currently-*linked* peers at auto-link evaluation time, not at
    /// insertion time (a name's linked status can change after the
    /// report landed).
    reported_by: BTreeSet<String>,
}

#[derive(Default)]
pub struct GossipTable {
    inner: RwLock<HashMap<String, Candidate>>,
}

impl GossipTable {
    /// Upserts one candidate. `reporter` is `None` for a self-announcement
    /// (the candidate telling us about itself directly) and `Some(name)`
    /// when relayed inside another instance's `known_peers` — either way
    /// the caller must have already independently verified this candidate
    /// really answers at `web_base_url` before calling this (see
    /// `web::gossip`'s own doc comment): this table never trusts an
    /// unverified entry.
    pub fn report(&self, name: &str, web_base_url: &str, git_base_url: &str, reporter: Option<&str>) {
        if name.trim().is_empty() || web_base_url.trim().is_empty() {
            return;
        }
        let now = now_secs();
        let mut map = self.inner.write().expect("gossip table lock poisoned");
        let entry = map.entry(name.to_string()).or_insert_with(|| Candidate {
            web_base_url: web_base_url.to_string(),
            git_base_url: git_base_url.to_string(),
            first_seen: now,
            last_seen: now,
            reported_by: BTreeSet::new(),
        });
        // The name is self-chosen, so corroboration only ever vouches for the
        // URLs it was gathered against — otherwise anyone could announce a
        // well-corroborated candidate's name with their own URL and inherit
        // its sources and age straight into an auto-link.
        if entry.web_base_url != web_base_url || entry.git_base_url != git_base_url {
            entry.web_base_url = web_base_url.to_string();
            entry.git_base_url = git_base_url.to_string();
            entry.first_seen = now;
            entry.reported_by.clear();
        }
        entry.last_seen = now;
        if let Some(reporter) = reporter {
            entry.reported_by.insert(reporter.to_string());
        }
        if map.len() > MAX_CANDIDATES
            && let Some(oldest) = map.iter().min_by_key(|(_, c)| c.last_seen).map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
    }

    /// Every candidate currently known — the anti-entropy push-pull
    /// payload both `/_federation/gossip/announce`'s response and
    /// `/_federation/gossip/peers` return.
    pub fn snapshot(&self) -> Vec<GossipCandidateWire> {
        self.inner
            .read()
            .expect("gossip table lock poisoned")
            .iter()
            .map(|(name, c)| GossipCandidateWire {
                name: name.clone(),
                web_base_url: c.web_base_url.clone(),
                git_base_url: c.git_base_url.clone(),
            })
            .collect()
    }

    /// A random-order sample of up to `n` known candidates — what each
    /// gossip tick re-announces against, so discovery propagates
    /// transitively instead of staying pinned to the original seeds.
    pub fn sample(&self, n: usize) -> Vec<GossipCandidateWire> {
        let mut all = self.snapshot();
        // Fisher-Yates, `rand::random` per swap — no need to pull in a
        // whole `rand::seq` shuffle for an at-most-500-element list.
        let len = all.len();
        for i in (1..len).rev() {
            let j = (rand::random::<u64>() as usize) % (i + 1);
            all.swap(i, j);
        }
        all.truncate(n);
        all
    }

    /// Not-yet-linked candidates whose corroboration already clears both
    /// bars — `reported_by` restricted to names that are *currently*
    /// linked peers (`linked_names`), evaluated now rather than cached,
    /// since a source's own linked status can change after it reported.
    /// Both conditions required: source-count alone is spammable in a
    /// burst from freshly-created sock-puppet "peers", age alone is
    /// satisfied by a single slow-dripping source.
    pub fn auto_link_candidates(
        &self,
        min_sources: u32,
        min_age_secs: u64,
        linked_names: &HashSet<String>,
    ) -> Vec<GossipCandidateWire> {
        let now = now_secs();
        self.inner
            .read()
            .expect("gossip table lock poisoned")
            .iter()
            .filter_map(|(name, c)| {
                if linked_names.contains(name) {
                    return None;
                }
                let qualifying = c.reported_by.iter().filter(|r| linked_names.contains(*r)).count() as u32;
                let old_enough = now.saturating_sub(c.first_seen) >= min_age_secs;
                (qualifying >= min_sources && old_enough).then(|| GossipCandidateWire {
                    name: name.clone(),
                    web_base_url: c.web_base_url.clone(),
                    git_base_url: c.git_base_url.clone(),
                })
            })
            .collect()
    }
}

/// One join/bootstrap round against `seed_url` (section 5 step 1) — POSTs
/// `self_info` plus a sample of what's already known, merges back whatever
/// the seed answers with (each entry independently re-verified first, same
/// as `web::gossip::handler_announce` does for an inbound announce).
async fn announce_once(gossip: &GossipTable, seed_url: &str, self_info: &GossipCandidateWire) {
    let sample = gossip.sample(32);
    match crate::instance_client::announce(seed_url, self_info, &sample).await {
        Ok(resp) => {
            for candidate in resp.known_peers {
                if candidate.name == self_info.name {
                    continue;
                }
                if crate::instance_client::verify_instance(&candidate.web_base_url)
                    .await
                    .is_ok()
                {
                    gossip.report(&candidate.name, &candidate.web_base_url, &candidate.git_base_url, None);
                }
            }
        }
        Err(e) => {
            tracing::warn!(target: "gitquarry_federation::gossip", seed_url, error = %format!("{e:#}"), "gossip announce failed")
        }
    }
}

/// Runs [`announce_once`] against every configured seed, once, at startup
/// — section 5 step 1. Best-effort per seed: one unreachable seed must
/// never stop the others, or stop federation from starting at all.
pub async fn bootstrap_seeds(gossip: &GossipTable, seed_peers: &[String], self_info: &GossipCandidateWire) {
    for seed in seed_peers {
        announce_once(gossip, seed, self_info).await;
    }
}

/// Checks every not-yet-linked candidate against the auto-link threshold
/// and promotes qualifying ones via the same [`crate::domain::peer::add`]
/// path a manual admin-add uses — section 5 step 5. A promotion failure
/// (the candidate went down between corroboration and promotion, a name
/// collision, ...) is logged and skipped, never fatal to the tick.
pub async fn run_auto_link_tick(
    backend: &dyn s3db::Backend,
    cipher: &s3db::Cipher,
    gossip: &GossipTable,
    min_sources: u32,
    min_age_secs: u64,
) {
    let linked: HashSet<String> = match crate::access::list_all_peers(backend, cipher).await {
        Ok(peers) => peers.into_iter().map(|p| p.name).collect(),
        Err(e) => {
            tracing::warn!(target: "gitquarry_federation::gossip", error = %format!("{e:#}"), "auto-link tick couldn't list current peers");
            return;
        }
    };
    for candidate in gossip.auto_link_candidates(min_sources, min_age_secs, &linked) {
        match crate::domain::peer::add(
            backend,
            cipher,
            &candidate.name,
            &candidate.web_base_url,
            &candidate.git_base_url,
            crate::entities::PeerAccessLevel::BrowseOnly,
            true,
        )
        .await
        {
            Ok(Some(_)) => {
                tracing::info!(
                    target: "gitquarry_federation::gossip",
                    peer = %candidate.name,
                    min_sources,
                    min_age_secs,
                    "auto-linked after gossip corroboration"
                );
            }
            Ok(None) => {} // Name collision with an already-linked peer added concurrently — fine, skip.
            Err(e) => tracing::warn!(
                target: "gitquarry_federation::gossip",
                peer = %candidate.name,
                error = %format!("{e:#}"),
                "auto-link failed"
            ),
        }
    }
}

/// Runs [`announce_once`] against a fresh random sample every
/// `interval_secs`, then [`run_auto_link_tick`] — section 5 steps 3 and 5,
/// on the same tick (the doc's own "el refresh de lista de repos... pasa a
/// correr en el mismo tick de gossip" reasoning: one timer, not several
/// independently-drifting ones).
pub async fn run_periodic_tick(
    state: std::sync::Arc<crate::state::FederationState>,
    interval_secs: u64,
    auto_link_min_sources: u32,
    auto_link_min_age_secs: u64,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs.max(1)));
    loop {
        ticker.tick().await;
        if let Some(self_info) = &state.self_announce {
            let sample = state.gossip.sample(8);
            for peer in sample {
                announce_once(&state.gossip, &peer.web_base_url, self_info).await;
            }
        }
        run_auto_link_tick(
            state.backend.as_ref(),
            &state.cipher,
            &state.gossip,
            auto_link_min_sources,
            auto_link_min_age_secs,
        )
        .await;
    }
}
