//! Library root for the `gitquarry` binary. Every module lives here, `pub`,
//! so a separate presentation-layer crate (`gitquarry-webui`, wired back in
//! from `main.rs`) can depend on this crate instead of reaching into private
//! internals — a real compile-time boundary, not a cosmetic file move.
//!
//! `main.rs` is just the CLI dispatch shim; see its own doc comment for the
//! full command list.

pub mod app;
pub mod auth;
pub mod backup;
pub mod config;
pub mod crypto;
pub mod domain;
pub mod events;
pub mod git;
pub mod mirror;
pub mod storage;
pub mod validate;
