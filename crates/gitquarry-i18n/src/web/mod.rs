//! Page-template copy catalog: nav chrome, headings, button labels, table
//! headers, placeholders, prose — the actual rendered page content, as
//! opposed to [`super::shared`]'s cross-cutting error/flash messages. Split
//! one submodule per page file (mirroring `src/web/`) instead of one flat
//! file, since template copy is one-off-per-page by nature (no
//! duplicate-collapse leverage the way `shared` had) and a single file would
//! grow unmanageably large across ~30 page files.
//!
//! [`auth`]/[`repo`] group each crate-side module's pages into their own
//! subdirectory, mirroring that module's own layout; [`search`]/[`settings`]
//! stay one flat file since each backs a single page. Everything lives here
//! regardless of which crate renders it — no i18n catalog is allowed to
//! live outside this crate.

pub mod audit;
pub mod auth;
pub mod chrome;
pub mod repo;
pub mod repo_new;
pub mod repo_settings;
pub mod search;
pub mod settings;
