//! Plain serde DTOs shared across crates — deliberately minimal, no
//! business logic. Anything single-sourced in one crate (`Permission`,
//! `User`, `OwnerRef`, ...) does not belong here.

pub mod ipc;
mod merge_request;
mod pipeline;
mod uid;

pub use merge_request::{
    CommentAnchor, DiffSide, MergeRequestMeta, MergeStrategy, MrComment, MrReview, MrStatus, ReviewVerdict,
};
pub use pipeline::{JobGraph, JobKind, JobSpec, PipelineTrigger, SecretRef};
pub use uid::Uid;

/// Bumped whenever a DTO in this crate changes shape in a way that isn't
/// backward-compatible (a field removed/renamed/re-typed — adding a new
/// `#[serde(default)]` field doesn't count). Sent as [`WIRE_VERSION_HEADER`]
/// on every loopback call between core and an optional module, in both
/// directions, and checked by both sides' `require_internal_token`
/// middleware — a mismatch means one binary got upgraded without the
/// other, which would otherwise show up as a confusing serde failure deep
/// inside some specific route instead of one clear "these two don't speak
/// the same wire version" error at the door.
pub const WIRE_PROTOCOL_VERSION: u32 = 1;

/// Header carrying [`WIRE_PROTOCOL_VERSION`] — same casing convention
/// `x-internal-token` already uses for this same loopback traffic.
pub const WIRE_VERSION_HEADER: &str = "x-gitquarry-wire-version";
