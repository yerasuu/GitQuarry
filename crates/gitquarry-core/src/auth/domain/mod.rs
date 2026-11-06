//! `User`/`Group` mutation CRUD, split out of the top-level `crate::domain`
//! (which stays for `Repo`/`MergeRequest`/`Release` — kernel concepts touched
//! by `webui`, `git`, and `federation` alike) because these two are auth-owned:
//! [`crate::auth::entities`] already holds the `User`/`Group` struct defs
//! themselves, so this just completes that ownership with the CRUD that
//! mutates them. Same one-file-per-concern shape `crate::domain` uses.

pub mod group;
pub mod passkey;
pub mod pat;
pub mod user;
