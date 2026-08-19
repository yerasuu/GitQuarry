//! CI/CD, split into its own process — see `docs/ci-cd-concept.md`. Talks
//! to `gitquarry-core` only over loopback HTTP ([`core_client`]), same
//! posture `gitquarry-federation` already established for its own core
//! leg. Phase 1 skeleton only: config/storage/loopback-server wiring, no
//! pipeline graph, execution engine, or event-bus trigger consumer yet —
//! those are later phases in the doc's own sequencing.
//!
//! Both a lib and a bin target (this file plus `main.rs`), same reasoning
//! `gitquarry-federation`'s own split gives: pieces landing commit-by-commit
//! can be fully implemented and unit-tested before `main.rs` wires them
//! into an actual running server.

pub mod config;
pub mod core_client;
pub mod graph;
pub mod state;
pub mod web;
