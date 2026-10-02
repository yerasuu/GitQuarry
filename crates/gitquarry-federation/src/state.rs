//! Shared state for every federation web handler — this crate's own,
//! deliberately unrelated to `gitquarry-core`'s `AppContext` (no shared
//! type between the two processes beyond `gitquarry-wire`'s DTOs).

use crate::core_client::CoreClient;
use crate::gossip::GossipTable;
use crate::rate_limit::PeerRateLimiter;
use s3db::{Backend, BackendConfig, Cipher};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct FederationState {
    pub backend: Arc<dyn Backend>,
    pub cipher: Arc<Cipher>,
    pub core: CoreClient,
    /// Kept around only for `web::api::bucket_label` — this instance's
    /// `/_api/instance` discovery `name`, same role
    /// `gitquarry-core`'s own `AppContext::bucket` plays.
    pub backend_config: BackendConfig,
    /// Gossip candidates (section 5) — separate from the `Peer` collection
    /// in `backend`/`cipher`'s storage: this is in-memory-only, unverified
    /// until independently checked, never itself browsable/forkable.
    pub gossip: Arc<GossipTable>,
    /// `None` unless `gitquarry-federation.toml` sets both
    /// `own_web_base_url`/`own_git_base_url` — gossip bootstrap/outbound
    /// announce needs a `(name, web_base_url, git_base_url)` to tell other
    /// instances about *this* one; without it this instance can still
    /// answer incoming announces, just never initiate its own.
    pub self_announce: Option<crate::gossip::GossipCandidateWire>,
    pub rate_limiter: Arc<PeerRateLimiter>,
    pub rate_limit_public_max: u32,
    pub rate_limit_public_window: Duration,
    pub rate_limit_token_max: u32,
    pub rate_limit_token_window: Duration,
    /// `config::Config::rate_limit_peer_override_by_ip` — see that field's
    /// own doc comment on the attribution tradeoff this toggles.
    pub rate_limit_peer_override_by_ip: bool,
}

pub type State_ = axum::extract::State<Arc<FederationState>>;

/// The physical storage identifier (`LocalParams`/`S3Params`/etc.'s own
/// `bucket` field) — every driver's config shares that field name
/// specifically so a caller doesn't need a driver-specific match beyond
/// picking the field. Reused as this instance's discovery `name` in
/// `/_api/instance` and as its gossip self-announce name, same role
/// `gitquarry-core`'s own `AppContext::bucket` plays.
pub fn bucket_label(cfg: &BackendConfig) -> &str {
    match cfg {
        BackendConfig::S3(p) => &p.bucket,
        BackendConfig::R2(p) => &p.bucket,
        BackendConfig::Local(p) => &p.bucket,
    }
}
