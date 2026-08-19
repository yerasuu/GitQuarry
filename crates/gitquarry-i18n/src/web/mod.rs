//! Page-template copy catalog: nav chrome, headings, button labels, table
//! headers, placeholders, prose — the actual rendered page content, as
//! opposed to [`super::shared`]'s cross-cutting error/flash messages. Split
//! one submodule per page file (mirroring `src/web/`) instead of one flat
//! file, since template copy is one-off-per-page by nature (no
//! duplicate-collapse leverage the way `shared` had) and a single file would
//! grow unmanageably large across ~30 page files.
//!
//! `login`/`users`/`groups`/`security`/`profile`/`preferences`/`favorites`
//! live at [`crate::auth::i18n`] instead, alongside the [`crate::auth::web`]
//! pages they're copy for.

pub mod audit;
pub mod chrome;
pub mod repo_activity;
pub mod repo_board;
pub mod repo_compare;
pub mod repo_components;
pub mod repo_issues;
pub mod repo_log;
pub mod repo_milestones;
pub mod repo_new;
pub mod repo_pulls;
pub mod repo_refs;
pub mod repo_releases;
pub mod repo_settings;
pub mod repo_stats;
pub mod repo_summary;
pub mod repo_tree;
