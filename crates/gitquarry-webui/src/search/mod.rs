//! `/_search`: cross-repo filename and commit-message search, split out of
//! `crate::webui` since it's a self-contained feature with no domain entity
//! of its own (queries `gitquarry_core::git`/`gitquarry_core::storage`
//! directly) — same reasoning that pulled [`crate::auth`] out, just without
//! a `domain`/`access`/`entities` layer to go with it. Template copy lives in
//! `gitquarry_i18n::web::search`, not here.

pub mod web;
