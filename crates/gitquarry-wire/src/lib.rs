//! Plain serde DTOs shared across crates — deliberately minimal, no
//! business logic. Anything single-sourced in one crate (`Permission`,
//! `User`, `OwnerRef`, ...) does not belong here.

mod merge_request;
mod uid;

pub use merge_request::{
    CommentAnchor, DiffSide, MergeRequestMeta, MergeStrategy, MrComment, MrReview, MrStatus, ReviewVerdict,
};
pub use uid::Uid;
