//! External-forge ref/MR mirror sync — see `docs/external-mirror-concept.md`
//! for the feature itself and `docs/mirror-external-extraction-concept.md`
//! for why this is its own *crate* while staying a plain library, called
//! in-process rather than run as a separate binary/process: it's a code-
//! organization boundary only. `gitquarry-core` can't depend on it directly
//! (this crate depends on core, for `AppContext`/`git`/`storage` — the
//! reverse would cycle), so `gitquarry-server` — which already depends on
//! both — is the one place that calls into [`engine`] and wires
//! [`gitquarry_core::mirror::MirrorSyncHook`] with it at startup.

pub mod access;
pub mod domain;
pub mod engine;
pub mod git_remote;
mod mr_sync;
// Lives in the forge clients, whose `since` cutoff needs the same
// comparison and can't depend on this crate without a cycle.
pub use gitquarry_forge_clients::stamp;
pub mod validate;
