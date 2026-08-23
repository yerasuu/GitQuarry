//! `/_settings`: server-wide settings (`.conf/global.json` — network binds,
//! mirrors cache, OIDC, backup), split out of `crate::webui` for the same reason
//! [`crate::search`] was: self-contained, no domain entity of its own beyond
//! [`gitquarry_core::storage::entities::ServerSettings`] (a plain struct, CRUD
//! stays inline in [`web`] rather than a whole `domain` layer for two
//! functions). [`web::require_admin`] is the shared admin-page gate also used
//! by [`crate::auth::web::users`] and [`crate::webui::repo_settings`].
//! Template copy lives in `gitquarry_i18n::web::settings`, not here.

pub mod web;
