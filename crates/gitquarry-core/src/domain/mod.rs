//! Entity CRUD, centralized: every mutation of a Repo/MergeRequest goes
//! through here, not scattered across `webui::*` handlers — see each
//! submodule's own doc comment for what belongs here vs. what stays in the
//! caller (CSRF, session/actor identity, audit logging, HTTP status codes).
//!
//! `User`/`Group` CRUD lives at [`crate::auth::domain`] instead: `Repo`/
//! `MergeRequest` are kernel concepts `git::ssh`/`git::http`/`mirror::*` all
//! mutate directly, so there's no single feature to own them, but
//! `User`/`Group` are auth-owned (their entity defs already live at
//! [`crate::auth::entities`]) — deliberately not moved here alongside
//! these. `Release` CRUD lives in the separate `gitquarry-domain` crate
//! instead: unlike `repo`/`merge_request`/`commit_signature` here, nothing in
//! core's own git-serving path ever calls back into it, only
//! `webui`/`server` do.

pub mod commit_signature;
pub mod merge_request;
pub mod repo;
