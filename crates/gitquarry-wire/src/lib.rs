//! Plain serde DTOs shared across 2+ of the three boundaries a gitquarry
//! deployment can have (core-internal IPC, federation's public API, peer-to-
//! peer gossip) — deliberately minimal, no business logic. Anything that
//! stays single-sourced in one binary (`Permission`, `User`, `OwnerRef`,
//! `Peer`, `AccessToken`, `PeerOrigin`, ...) does not belong here; the other
//! side reaches it only through an explicit IPC call.

mod instance;
pub mod ipc;
mod merge_request;
mod repo;
mod uid;

pub use instance::InstanceInfo;
pub use merge_request::{
    CommentAnchor, DiffSide, MergeRequestMeta, MergeStrategy, MrComment, MrDetail, MrReview, MrStatus, ReviewVerdict,
};
pub use repo::RepoSummary;
pub use uid::Uid;

/// Bumped whenever a DTO in this crate changes shape in a way that isn't
/// backward-compatible (a field removed/renamed/re-typed — adding a new
/// `#[serde(default)]` field doesn't count). Sent as [`WIRE_VERSION_HEADER`]
/// on every core↔federation loopback call in both directions
/// (`gitquarry-federation::core_client::CoreClient` and
/// `gitquarry-webui::webui::federation_proxy::send`) and checked by both
/// sides' `require_internal_token` middleware — a mismatch means one binary
/// got upgraded without the other, which would otherwise show up as a
/// confusing serde failure deep inside some specific route instead of one
/// clear "these two don't speak the same wire version" error at the door.
pub const WIRE_PROTOCOL_VERSION: u32 = 1;

/// Header carrying [`WIRE_PROTOCOL_VERSION`] — same casing convention
/// `x-internal-token` already uses for this same loopback traffic.
pub const WIRE_VERSION_HEADER: &str = "x-gitquarry-wire-version";
