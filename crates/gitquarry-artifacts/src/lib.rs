//! Package registry (npm/raw) + CI build-artifact storage, one
//! content-addressed blob service, split into its own process — see
//! `docs/artifacts-concept.md`. Talks to `gitquarry-core` only over
//! loopback HTTP ([`core_client`]), same posture `gitquarry-federation`/
//! `gitquarry-ci` already establish. Phase 1 scope only: storage/domain
//! foundation (content-addressed blobs, package/version metadata) plus the
//! config/loopback-server skeleton — no npm/raw HTTP adapters yet (Phase
//! 2/3 in the doc's own sequencing), no ci↔artifacts handoff leg yet
//! either (needs `gitquarry-ci`'s own later phase first).
//!
//! Both a lib and a bin target (this file plus `main.rs`), same reasoning
//! `gitquarry-federation`'s own split gives.

pub mod config;
pub mod core_client;
pub mod domain;
pub mod state;
pub mod storage;
pub mod web;
