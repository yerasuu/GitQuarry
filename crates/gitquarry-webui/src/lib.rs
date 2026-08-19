//! Presentation layer: the read-only web UI (plus settings/search/auth
//! pages), split out of `gitquarry-core` into its own crate so the
//! frontend implementation can be swapped later without touching
//! git-serving code. Depends on `gitquarry-core`'s public API only — no
//! reaching into its internals — so a future replacement can act on this
//! crate alone.
//!
//! `gitquarry-server` wires [`webui::run_server`] into the same process as
//! the ssh:// server, so from a deployment standpoint this is still one
//! binary, one process — the crate boundary is compile-time, not a second
//! service to run.

pub mod auth;
pub mod search;
pub mod settings;
pub mod webui;
