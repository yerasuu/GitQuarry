//! Auth's own web handlers. The logged-in viewer's private account settings
//! all live under `/_account/*`, one section per sidebar entry
//! (`crate::webui::account_settings_page`): [`avatar`] (`/_account/profile`,
//! self-service avatar), [`appearance`] (`/_account/appearance`, text-size/
//! theme/density), [`security`] (`/_account/security`, password and 2FA),
//! [`keys`] (`/_account/keys`, SSH and GPG keys), [`favorites`]
//! (`/_account/favorites`, self-service starred repos + visibility toggle),
//! [`pat`] (`/_account/pat`, self-service local personal access tokens —
//! separate from federation's own fine-grained tokens at `/_account/tokens`).
//! [`favorites`] also owns the *public* read-only view at
//! `/_profile/<user>/favorites` — the one section reachable by someone
//! other than the account owner, see that module's own doc comment.
//! [`login`] (`/_login`, `/_login_oidc`, `/_callback`, `/_logout`, session
//! cookies), [`profile`] (`/_profile/<username>`, public repo listing),
//! [`users`] (`/_users`, admin), [`groups`] (`/_groups`, shared namespaces),
//! [`group`] (`/_group/<path>`, one group's own public page — repos,
//! subgroups, members).
//! Routed from [`crate::webui::run_server`]'s own route table, same as any
//! other `webui::*` handler module.

pub mod appearance;
pub mod avatar;
pub mod favorites;
pub mod group;
pub mod groups;
pub mod keys;
pub mod login;
pub mod pat;
pub mod profile;
mod reauth;
pub mod security;
pub mod users;
