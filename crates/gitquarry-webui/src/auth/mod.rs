//! Login/security/favorites/appearance/avatar/profile/users/groups page handlers
//! ([`web`]) — presentation for `gitquarry_core::auth`'s identity/permission
//! model, split into its own namespace here rather than mirroring core's full
//! `auth` module (which also has `access`/`domain`/`oidc`/`password`/`totp`/
//! `entities`: business logic that stays in core, used by `git::ssh` too, not
//! just this crate). Template copy lives in `gitquarry_i18n::web::auth_*`,
//! alongside every other page's — no i18n catalog lives outside that crate.

pub mod web;
