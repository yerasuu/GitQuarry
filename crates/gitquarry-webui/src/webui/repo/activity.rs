//! `?view=activity`: recent commits on whichever ref is being viewed
//! (`rev`, the repo's default branch unless a viewer switched away from
//! it) — pure git data, no [`gitquarry_core::storage::audit`] involved.
//! That's a deliberate split, not an oversight: `audit` is the bucket-wide
//! admin/security trail `/_audit` reads (config saves, permission grants,
//! star/watch toggles, mirror syncs, ...), while this page answers one
//! narrower question — "what code actually landed on this repo's main
//! line" — which the commit log already answers completely and correctly
//! on its own. Same visibility gate as every other view (`handler_repo`
//! already checked it before dispatching here) — unlike `/_audit`, this
//! isn't admin-only: it's this one repo's own activity, not the whole
//! bucket's.

use super::components::{commit_avatars, commit_table};
use anyhow::Result;
use gitquarry_core::app::AppContext;
use gitquarry_core::git;
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo::activity as t;
use maud::html;
use std::path::Path as FsPath;

/// How many commits show on the activity view — more than
/// [`super::RECENT_LOG_LIMIT`]'s summary-tab teaser, since this page's whole
/// point is "what's been happening lately", not a short preview.
const ACTIVITY_LOG_LIMIT: usize = 30;

pub(super) async fn render_activity(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    rev: &str,
    locale: Locale,
) -> Result<String> {
    let mut commits = git::browse::log(bare_repo, rev, ACTIVITY_LOG_LIMIT, None).await?;
    git::browse::mark_signed(bare_repo, &mut commits).await?;
    let avatars = commit_avatars(ctx, bare_repo, &commits).await;

    Ok(html! {
        h3 { (t::recent_commits_heading(locale)) }
        (commit_table(repo_name, &commits, &avatars, locale))
    }
    .into_string())
}
