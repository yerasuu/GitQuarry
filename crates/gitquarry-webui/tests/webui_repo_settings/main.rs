//! A repo's `?view=settings*` pages and the one `POST /{repo}` all of its
//! sub-forms share, driven through `webui::router` as a browser would.

mod danger_zone;
mod default_branch;
mod general;
mod labels;
mod lfs_limit;
mod mirrors;
mod move_repo;
mod permissions;
mod protected_branches;
mod publish_mirrors;
mod required_reviewers;
mod sections;
mod support;
mod topics;
