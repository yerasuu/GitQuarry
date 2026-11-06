//! Shared state for every tracker web handler — this crate's own,
//! deliberately unrelated to `gitquarry-core`'s `AppContext` (no shared type
//! between the two processes beyond `gitquarry-wire`'s DTOs), same posture
//! `gitquarry-ci`'s own `CiState` already establishes.

use crate::core_client::CoreClient;
use s3db::Backend;
use s3db_crypto::Cipher;
use std::sync::Arc;

#[derive(Clone)]
pub struct TrackerState {
    pub backend: Arc<dyn Backend>,
    pub cipher: Arc<Cipher>,
    pub core: CoreClient,
}

/// A repo, named just by its uid — the only thing tracker's own domain
/// layer ever needs to address one, unlike `gitquarry-core`'s
/// `ResolvedRepo` (which also carries a `Remote` handle tracker has no use
/// for: it never touches `RepoConfig`/manifests directly, only its own
/// issues/board/milestones collections).
#[derive(Debug, Clone)]
pub struct TrackerTarget {
    pub uid: gitquarry_wire::Uid,
}

pub type State_ = axum::extract::State<Arc<TrackerState>>;
