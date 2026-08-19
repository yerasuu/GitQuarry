//! Issue tracker + kanban board + milestones — its own binary, optional and
//! separately deployed, same posture `gitquarry-ci`/`gitquarry-federation`
//! already establish (`docs/tracker-binary-split-concept.md`). Core stays
//! fully usable without it: a deploy that doesn't configure `[tracker]`
//! just loses the tracker tab.
//!
//! Talks to `gitquarry-core` only over loopback HTTP
//! ([`core_client::CoreClient`]) — never links `gitquarry-core` as a Rust
//! dependency at all. `gitquarry-webui`'s issues/board/milestones pages are
//! this crate's only caller, through the same kind of loopback client
//! (`ctx.tracker`).

pub mod config;
pub mod core_client;
pub mod domain;
pub mod i18n;
pub mod state;
pub mod storage;
pub mod web;
