//! Plain serde DTOs shared across crates — deliberately minimal, no
//! business logic. Anything single-sourced in one crate (`Permission`,
//! `User`, `OwnerRef`, ...) does not belong here.

mod pull_request;
mod uid;

pub use pull_request::{
    CommentAnchor, DiffSide, MergeStrategy, PrComment, PrDetail, PrReview, PrStatus, PullRequestMeta, ReviewVerdict,
};
pub use uid::Uid;
