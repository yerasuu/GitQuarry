//! `/_settings`: server-wide settings (`.conf/global.json` — network binds,
//! mirrors cache, OIDC), split out of `crate::webui`/`gitquarry_i18n::web` for
//! the same reason [`crate::search`] was: self-contained, no domain entity
//! of its own beyond [`gitquarry_core::storage::entities::ServerSettings`] (a plain
//! struct, CRUD stays inline in [`web`] rather than a whole `domain` layer
//! for two functions). [`webui::require_admin`] is the shared admin-page gate
//! also used by [`crate::auth::web::users`] and [`crate::webui::repo_settings`].

pub mod i18n;
pub mod web;
