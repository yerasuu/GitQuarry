//! Plain serde DTOs crossing the core↔tracker loopback IPC boundary —
//! deliberately minimal, no business logic. Anything that stays
//! single-sourced in one binary (`Permission`, `User`, `OwnerRef`, ...) does
//! not belong here; the other side reaches it only through an explicit IPC
//! call.

pub mod ipc;
mod merge_request;
pub mod tracker;
mod uid;

pub use merge_request::{
    CommentAnchor, DiffSide, MergeRequestMeta, MergeStrategy, MrComment, MrReview, MrStatus, ReviewVerdict,
};
pub use uid::Uid;

/// Bumped whenever a DTO in this crate changes shape in a way that isn't
/// backward-compatible (a field removed/renamed/re-typed — adding a new
/// `#[serde(default)]` field doesn't count). Sent as [`WIRE_VERSION_HEADER`]
/// on every core↔tracker loopback call in both directions
/// (`gitquarry-tracker::core_client::CoreClient` and
/// `gitquarry-webui::webui::tracker_client`) and checked by both
/// sides' `require_internal_token` middleware — a mismatch means one binary
/// got upgraded without the other, which would otherwise show up as a
/// confusing serde failure deep inside some specific route instead of one
/// clear "these two don't speak the same wire version" error at the door.
pub const WIRE_PROTOCOL_VERSION: u32 = 1;

/// Header carrying [`WIRE_PROTOCOL_VERSION`] — same casing convention
/// `x-internal-token` already uses for this same loopback traffic.
pub const WIRE_VERSION_HEADER: &str = "x-gitquarry-wire-version";
