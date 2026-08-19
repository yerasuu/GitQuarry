//! Login/security/favorites/appearance/avatar/profile/users/groups page handlers
//! ([`web`]) and their template copy ([`i18n`]) — presentation for
//! `gitquarry_core::auth`'s identity/permission model, split into its own
//! namespace here rather than mirroring core's full `auth` module (which
//! also has `access`/`domain`/`oidc`/`password`/`totp`/`entities`: business
//! logic that stays in core, used by `git::ssh` too, not just this crate).

pub mod i18n;
pub mod web;
