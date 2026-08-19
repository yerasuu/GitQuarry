//! Shared state for every artifacts web handler — this crate's own,
//! deliberately unrelated to `gitquarry-core`'s `AppContext` (no shared type
//! between the two processes beyond `gitquarry-wire`'s DTOs), same posture
//! `gitquarry-federation`'s own `FederationState` already establishes.

use crate::core_client::CoreClient;
use s3db::{Backend, Cipher};
use std::sync::Arc;

#[derive(Clone)]
pub struct ArtifactsState {
    pub backend: Arc<dyn Backend>,
    pub cipher: Arc<Cipher>,
    pub core: CoreClient,
}

pub type State_ = axum::extract::State<Arc<ArtifactsState>>;
