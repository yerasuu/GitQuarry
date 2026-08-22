//! Identity and permissions: who a request is (SSH pubkey, OIDC email, or
//! local username+password) and what they're allowed to touch.
//!
//! - [`access`]: the persisted model — `.conf/access.json` in the bucket —
//!   `User`, `Permission`, and the lookups `ssh`/`auth::web::login` resolve a
//!   pubkey/email/username against.
//! - [`domain`]: `User`/`Group` mutation CRUD — create/delete/set_password/
//!   add_pubkey/... — the actual writes, kept separate from `access`'s reads
//!   and lookups.
//!
//! OIDC client construction, password hashing/verification, and TOTP
//! generation/verification live in the standalone `gitquarry-auth-crypto`
//! crate — self-contained primitives with no dependency on this crate's own
//! `AppContext`/storage, consumed directly as `gitquarry_auth_crypto::{oidc,
//! password, totp}`.
//!
//! The login/security/favorites/preferences/profile/users/groups page
//! handlers (formerly `web` here) now live in the `gitquarry-webui` crate,
//! as `auth::web` there — presentation, not identity/permission logic.
//! Their template copy lives in `gitquarry-i18n`'s `web::auth_*`, alongside
//! every other page's.

pub mod access;
pub mod domain;
pub mod entities;
