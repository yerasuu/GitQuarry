//! Identity and permissions: who a request is (SSH pubkey, OIDC email, or
//! local username+password) and what they're allowed to touch.
//!
//! - [`access`]: the persisted model — `.conf/access.json` in the bucket —
//!   `User`, `Permission`, and the lookups `ssh`/`auth::web::login` resolve a
//!   pubkey/email/username against.
//! - [`domain`]: `User`/`Group` mutation CRUD — create/delete/set_password/
//!   add_pubkey/... — the actual writes, kept separate from `access`'s reads
//!   and lookups.
//! - [`federation_token`]: resolves a federation-issued fine-grained token
//!   for git-smart-HTTP Basic-Auth — [`crate::git::http`]'s only outbound
//!   call into `gitquarry-federation`'s loopback API, kept separate from
//!   `webui::federation_client`'s browse-page lookups since it's needed by
//!   git-serving itself, not presentation.
//!
//! OIDC client construction, password hashing/verification, and TOTP
//! generation/verification live in the standalone `gitquarry-auth-crypto`
//! crate — self-contained primitives with no dependency on this crate's own
//! `AppContext`/storage, consumed directly as `gitquarry_auth_crypto::{oidc,
//! password, totp}`.
//!
//! The login/security/favorites/preferences/profile/users/groups page
//! handlers (formerly `web` here) and their template copy (formerly `i18n`
//! here) now live in the `gitquarry-webui` crate, as `auth::web`/`auth::i18n`
//! there — presentation, not identity/permission logic.

pub mod access;
pub mod domain;
pub mod entities;
pub mod federation_token;
