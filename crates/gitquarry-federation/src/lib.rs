//! Cross-instance federation, split out of `gitquarry-core` into its own
//! process — see `docs/federation-binary-split-concept.md`. Talks to core
//! only over loopback HTTP ([`core_client`]); talks to *other* gitquarry
//! instances over their own public federation API ([`instance_client`]).
//! [`entities`] holds the two storage-backed types this binary owns
//! ([`entities::Peer`], [`entities::AccessToken`]); [`access`] is their CRUD
//! layer; [`domain`] is every mutation of them, transport-agnostic.
//!
//! Both a lib and a bin target (this file plus `main.rs`) so the pieces
//! landing commit-by-commit (`docs/federation-binary-split-concept.md`
//! section 10) can be fully implemented and unit-tested before `main.rs`
//! wires them into an actual running server.

pub mod access;
pub mod config;
pub mod core_client;
pub mod domain;
pub mod entities;
pub mod gossip;
pub mod instance_client;
pub mod rate_limit;
pub mod sister;
pub mod state;
pub mod time;
pub mod validate;
pub mod web;
