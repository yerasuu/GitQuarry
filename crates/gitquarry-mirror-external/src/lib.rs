//! External-forge ref/PR mirror sync — see `docs/external-mirror-concept.md`
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
// Real code never needs `pr_sync` public: it's only called from within
// `engine`. Widened here so `tests/pr_sync.rs` (a real integration test
// crate, per the repo-wide tests/ policy) can reach it — see
// `gitquarry_core::git`'s `lfs` for the same pattern.
#[cfg(any(test, feature = "test-util"))]
pub mod pr_sync;
#[cfg(not(any(test, feature = "test-util")))]
mod pr_sync;
pub mod stamp;
pub mod validate;
