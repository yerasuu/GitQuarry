//! `?view=activity`: recent commits plus recent repo-scoped events (forks,
//! branch/tag/release/MR lifecycle, settings/permission changes — anything
//! [`gitquarry_core::storage::audit`] recorded with this repo as `target`,
//! minus [`LOW_VALUE_ACTIONS`]) side by side. Same visibility gate as every
//! other view (`handler_repo` already checked it before dispatching here)
//! — unlike `/_audit`, this isn't admin-only: it's this one repo's own
//! activity, not the whole bucket's.

use super::components::{commit_avatars, commit_table};
use anyhow::Result;
use gitquarry_core::app::AppContext;
use gitquarry_core::git;
use gitquarry_core::storage::audit::{self, AuditEntry};
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::repo_activity as t;
use maud::{Markup, html};
use std::path::Path as FsPath;

/// How many commits show on the activity view — more than
/// [`super::RECENT_LOG_LIMIT`]'s summary-tab teaser, since this page's whole
/// point is "what's been happening lately", not a short preview.
const ACTIVITY_LOG_LIMIT: usize = 30;

/// Excluded from the activity feed as low-value: `repo.star`/`unstar`/
/// `watch`/`unwatch` are personal-preference toggles that never change the
/// repo itself, `mirror.sync` is infra rather than human/content activity
/// (it's also only ever logged for a manual trigger, never the periodic
/// background job — see `webui::repo_settings::mirrors`'s own call site —
/// but even a deliberate one is still "an admin poked a sync job", not
/// "something happened to this repo"), and `merge_request.comment` is
/// high-volume, low-signal for a quick "what happened here" glance (the MR
/// itself still shows up via every other `merge_request.*` action). None of
/// these are dropped from the write path or `/_audit` — only hidden from
/// this one feed.
const LOW_VALUE_ACTIONS: &[&str] = &[
    "repo.star",
    "repo.unstar",
    "repo.watch",
    "repo.unwatch",
    "mirror.sync",
    "merge_request.comment",
];

#[cfg(any(test, feature = "test-util"))]
pub fn filter_low_value(events: Vec<AuditEntry>) -> Vec<AuditEntry> {
    filter_low_value_impl(events)
}
#[cfg(not(any(test, feature = "test-util")))]
fn filter_low_value(events: Vec<AuditEntry>) -> Vec<AuditEntry> {
    filter_low_value_impl(events)
}
fn filter_low_value_impl(mut events: Vec<AuditEntry>) -> Vec<AuditEntry> {
    events.retain(|e| !LOW_VALUE_ACTIONS.contains(&e.action.as_str()));
    events
}

pub(super) async fn render_activity(
    ctx: &AppContext,
    bare_repo: &FsPath,
    repo_name: &str,
    rev: &str,
    locale: Locale,
) -> Result<String> {
    let commits = git::browse::log(bare_repo, rev, ACTIVITY_LOG_LIMIT, None).await?;
    let avatars = commit_avatars(ctx, bare_repo, &commits).await;
    let events = filter_low_value(audit::list_recent_for(&ctx.backend(), &ctx.cipher, repo_name).await?);

    Ok(html! {
        h3 { (t::recent_commits_heading(locale)) }
        (commit_table(repo_name, &commits, &avatars, locale))
        h3 { (t::events_heading(locale)) }
        (render_events(&events, locale))
    }
    .into_string())
}

#[cfg(any(test, feature = "test-util"))]
pub fn render_events(events: &[AuditEntry], locale: Locale) -> Markup {
    html! {
        div class="table-scroll" {
        table class="list" {
            tr { th { (t::when_column(locale)) } th { (t::who_column(locale)) } th { (t::action_column(locale)) } th { (t::detail_column(locale)) } }
            @if events.is_empty() {
                tr { td colspan="4" { em { (t::no_recent_events_hint(locale)) } } }
            }
            @for e in events {
                tr {
                    td class="muted" { (e.at) }
                    td { (e.actor) }
                    td { code { (e.action) } }
                    td class="muted" { (e.detail.as_deref().unwrap_or("")) }
                }
            }
        }
        }
    }
}
#[cfg(not(any(test, feature = "test-util")))]
fn render_events(events: &[AuditEntry], locale: Locale) -> Markup {
    html! {
        div class="table-scroll" {
        table class="list" {
            tr { th { (t::when_column(locale)) } th { (t::who_column(locale)) } th { (t::action_column(locale)) } th { (t::detail_column(locale)) } }
            @if events.is_empty() {
                tr { td colspan="4" { em { (t::no_recent_events_hint(locale)) } } }
            }
            @for e in events {
                tr {
                    td class="muted" { (e.at) }
                    td { (e.actor) }
                    td { code { (e.action) } }
                    td class="muted" { (e.detail.as_deref().unwrap_or("")) }
                }
            }
        }
        }
    }
}
