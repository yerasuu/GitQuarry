//! Product-facing functionality that needs `gitquarry-core`'s kernel
//! (`AppContext`/storage/git) but that the kernel itself never needs back —
//! the layer between `gitquarry-core` and its consumers (`gitquarry-webui`,
//! `gitquarry-server`), same position `gitquarry-webui` itself occupies
//! relative to `gitquarry-server`.
//!
//! [`release`]: release CRUD + asset upload/signature-verify, transport-
//! agnostic (`anyhow::Result`, no CSRF/session/audit concerns — those stay
//! in the caller). Only `webui`/`server` ever call it; unlike
//! `gitquarry_core::domain::{repo,pull_request,remote_fork}`, nothing in
//! core's own git-serving path (`git::ssh`/`git::http`/`mirror::hook`) calls
//! back into it, so it doesn't have to live in core itself.
//!
//! [`commands`]: human-run operator maintenance (`gitquarry <command> ...`)
//! — `fsck`/`repack`/`backfill-repo-index`. Only `gitquarry-server`'s CLI
//! dispatcher calls these.

pub mod commands;
pub mod release;
