//! Named users and their per-repo SSH permissions, plus
//! [`crate::auth::entities::Group`]s. One object per user/group under
//! `.conf/users/`/`.conf/groups/`, plus hint-indexes (`.conf/index/...`)
//! for the lookups that have to stay fast regardless of user count:
//! resolving an SSH pubkey fingerprint or an OIDC email to a username, and
//! reserving a namespace across both users and groups at once (see
//! [`namespace::reserve_namespace`]'s doc comment for why that's a
//! separate CAS domain from either). The web UI manages this at `/_users`
//! (users) and `/_groups` (groups); the SSH server ([`crate::git::ssh`])
//! queries it to authenticate and authorize.
//!
//! Every index object here is a *hint, never a fact*: whatever it points at
//! is re-verified before being trusted, and a hint that fails verification
//! is stale and safely reclaimable, never fatal — see
//! [`pubkey::resolve_by_fingerprint`] and [`pubkey::claim_pubkey`] for what
//! that looks like in practice.
//!
//! Permissions attach to a [`crate::auth::entities::User`], not to any
//! single key: losing a laptop means removing that one pubkey from the
//! user, not re-granting every repo to a replacement key. A pubkey belongs
//! to at most one user.
//!
//! A repo's namespace (the segment before its first `/`) is either a
//! username or a [`crate::auth::entities::Group`] name, never both —
//! [`namespace::reserve_namespace`] enforces that at creation time, both
//! for new users and new groups, since
//! [`permissions::effective_permission_for`] needs the two to stay
//! unambiguous: it can't tell "the `acme` group" from "the `acme` user"
//! apart other than by which store actually has that name in it.
//!
//! One file per concern (mirrors the layout [`crate::git::browse`] uses for
//! its own per-concern split): [`users`]/[`groups`] (CRUD), [`permissions`]
//! (the actual grant-resolution logic), [`pubkey`] (SSH fingerprint index),
//! [`namespace`] (the shared user/group name reservation), [`repo_index`]
//! (per-repo permission-grant index), [`login_throttle`] (password-login
//! counter). Every `pub` item each one declares is re-exported here so
//! every existing `access::xxx` call site keeps working unchanged.
//!
//! Linked-peer CRUD, fine-grained personal access tokens, and peer
//! credentials (federation layers 1/4) live in the separate
//! `gitquarry-federation` process now, not this crate at all — see
//! `docs/federation-binary-split-concept.md`.

mod email;
mod groups;
mod login_throttle;
mod namespace;
// pub only for test/test-util builds, same reasoning as `permissions`/
// `pubkey` just below.
#[cfg(any(test, feature = "test-util"))]
pub mod quota;
#[cfg(not(any(test, feature = "test-util")))]
mod quota;
// pub only for test/test-util builds so external `tests/*.rs` integration
// tests can name `gitquarry_core::auth::access::permissions::*` directly.
#[cfg(any(test, feature = "test-util"))]
pub mod permissions;
#[cfg(not(any(test, feature = "test-util")))]
mod permissions;
// pub only for test/test-util builds so external `tests/*.rs` integration
// tests can name `gitquarry_core::auth::access::pubkey::*` directly.
#[cfg(any(test, feature = "test-util"))]
pub mod pubkey;
#[cfg(not(any(test, feature = "test-util")))]
mod pubkey;
// pub only for test/test-util builds so external `tests/*.rs` integration
// tests can name `gitquarry_core::auth::access::pat::*` directly.
#[cfg(any(test, feature = "test-util"))]
pub mod pat;
#[cfg(not(any(test, feature = "test-util")))]
mod pat;
mod repo_index;
mod users;

pub use email::*;
pub use groups::*;
pub use login_throttle::*;
pub use namespace::*;
pub use pat::*;
pub use permissions::*;
pub use pubkey::*;
pub use quota::*;
pub use repo_index::*;
pub use users::*;
