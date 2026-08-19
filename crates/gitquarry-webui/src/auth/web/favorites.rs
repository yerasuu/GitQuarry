//! Split in two: `/_profile/<user>/favorites` (this file's
//! [`handler_favorites_get`]) is the public read-only view — reachable by
//! anyone, including anonymous, once `<user>` opts into `favorites_public`
//! (see [`super::appearance`]... no, see the visibility toggle right on
//! [`render_favorites_page`] below now) — while `/_account/favorites` (this
//! file's [`handler_account_favorites_get`]/[`handler_account_favorites_post`])
//! is the self-service editable list + visibility toggle, sidebar-nested
//! under `crate::webui::account_settings_page` like every other `/_account/*`
//! section. `<user>`'s own session hitting the public URL gets redirected
//! to the account one rather than rendered twice.
//!
//! Starring is a personal bookmark list, not an account-identity setting —
//! same [`gitquarry_core::auth::access::User::starred`] field
//! `render_star_button` (in [`crate::webui::actions`]) already writes to;
//! this is just its own page plus the toggle action.
//!
//! Read-only means the starred list filtered through [`can_view_repo`] same
//! as [`super::profile`]'s own repo listing: a private repo `<user>` starred
//! must not leak to a viewer who has no grant on it, even though `<user>`'s
//! own view of this same list skips that check (their own star list, their
//! own call).

use super::login;
use crate::webui::repo::can_view_repo;
use crate::webui::{
    AppError, Flash, State_, account_settings_page, csrf_field, effective_density, effective_locale, effective_theme,
    is_local_redirect, layout, render_flash, require_csrf, require_login, settings_section, url_encode,
};
use axum::extract::{Form, Path, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::app::AppContext;
use gitquarry_core::auth::access;
use gitquarry_core::auth::entities::User;
use gitquarry_core::domain;
use gitquarry_core::storage::{self, Cas};
use maud::{Markup, PreEscaped, html};
use std::sync::Arc;
use tokio::task::JoinSet;

/// `GET /_profile/<user>/favorites` — public read-only view, or a redirect
/// to `/_account/favorites` if `<user>` is the logged-in session's own.
pub(crate) async fn handler_favorites_get(
    State(ctx): State_,
    Path(username): Path<String>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let viewer = login::current_user(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;

    if let Some(me) = &viewer
        && me.username == username
    {
        return Ok(Redirect::to("/_account/favorites").into_response());
    }

    // Not our own session's username: only reachable at all if that user
    // opted into `favorites_public` — same not-found-vs-forbidden split
    // `can_view_repo`'s callers use elsewhere, though the username itself
    // isn't a secret here (`/_profile/<user>` already publishes it). Only
    // this one user's own record is needed here, not the whole bucket.
    let target = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &username)
        .await?
        .0
        .ok_or_else(|| {
            AppError::not_found(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::user_not_found(locale, &username)
            ))
        })?;
    if !target.favorites_public {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::favorites_not_public(locale, &username)
        )));
    }
    let visible = visible_starred(&ctx, &headers, &target.starred).await?;
    let body = render_public_favorites_page(&username, &visible, locale).into_string();
    let topnav = login::topnav(&ctx, &headers).await;
    Ok(Html(layout(
        "favorites",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// Concurrently fetches each starred uid's `RepoConfig`, dropping anything
/// that fails to fetch or no longer exists (deleted since being starred —
/// there's no name left to show at all now that `User::starred` is
/// uid-keyed, unlike when it stored a name that could still be displayed
/// even if the repo behind it was gone). Shared by [`own_starred_with_counts`]
/// and [`visible_starred`] (which additionally drops anything that fails
/// `can_view_repo`).
async fn fetch_repo_configs(
    ctx: &AppContext,
    repos: impl IntoIterator<Item = gitquarry_core::storage::uid::Uid>,
) -> Vec<gitquarry_core::storage::entities::RepoConfig> {
    let mut set = JoinSet::new();
    for uid in repos {
        let backend = ctx.backend();
        let cipher = Arc::clone(&ctx.cipher);
        set.spawn(async move { storage::settings::get_repo_config(backend.as_ref(), uid.as_str(), &cipher).await });
    }
    let mut out = Vec::new();
    while let Some(result) = set.join_next().await {
        if let Ok(Ok((config, Some(_etag)))) = result {
            out.push(config);
        }
    }
    out
}

/// Every one of `starred`'s own repos (that still exist), paired with its
/// current `RepoConfig::star_count` — unlike [`visible_starred`], nothing
/// else is filtered out here: this is the owner's own list, shown in full
/// same as before (own star list, own call).
async fn own_starred_with_counts(
    ctx: &AppContext,
    starred: &std::collections::BTreeSet<gitquarry_core::storage::uid::Uid>,
) -> Vec<(String, u32)> {
    let mut pairs: Vec<(String, u32)> = fetch_repo_configs(ctx, starred.iter().cloned())
        .await
        .into_iter()
        .map(|c| (c.full_name, c.star_count))
        .collect();
    pairs.sort();
    pairs
}

/// `starred` filtered down to what `headers`' viewer (anonymous or not) can
/// actually see, same per-repo `can_view_repo` check
/// [`super::profile::list_owned_repos`] uses — a starred private repo the
/// profile owner can see must not leak to someone with no grant on it just
/// because it showed up on a public favorites list. Pairs each visible name
/// with its `RepoConfig::star_count`, already fetched here for the
/// visibility check — no separate `access.json` fetch needed just for that.
async fn visible_starred(
    ctx: &AppContext,
    headers: &HeaderMap,
    starred: &std::collections::BTreeSet<gitquarry_core::storage::uid::Uid>,
) -> Result<Vec<(String, u32)>, AppError> {
    let mut visible = Vec::new();
    for config in fetch_repo_configs(ctx, starred.iter().cloned()).await {
        if can_view_repo(ctx, headers, &config.full_name, config.visibility).await? {
            visible.push((config.full_name, config.star_count));
        }
    }
    visible.sort();
    Ok(visible)
}

pub(crate) async fn handler_account_favorites_get(
    State(ctx): State_,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/favorites").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    let starred = own_starred_with_counts(&ctx, &me.starred).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let content = render_favorites_page(&me, &csrf_field(&headers), None, &starred, locale);
    let body = account_settings_page("favorites", content, locale).into_string();
    Ok(Html(layout(
        "favorites",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct FavoritesForm {
    // Wire name `star_action`, not `action` -- see `actions.rs`'s
    // `star_button_html` doc comment on why the async star button's own
    // hidden field had to move off that name. `render_favorites_page`'s
    // plain (non-`data-async`) "remove" form doesn't need the rename for
    // its own sake, but has to match since both forms feed this struct.
    #[serde(default)]
    star_action: String,
    /// Set only by the visibility-toggle form on this same page — mutually
    /// exclusive with `star_action`, same "two possible forms, one struct"
    /// shape [`super::security::SecurityForm`] uses for its own several
    /// sub-forms.
    #[serde(default)]
    favorites_action: String,
    #[serde(default)]
    repo: String,
    #[serde(default)]
    csrf: String,
    /// Checkbox (`"1"` when checked, absent/empty otherwise) — only read
    /// when `favorites_action == "set_visibility"`.
    #[serde(default)]
    favorites_public: String,
    /// Where to redirect after toggling a star, if set — the star button on
    /// a repo page sets this to that repo's own URL so toggling doesn't
    /// bounce the viewer back to this page; omitted (as when toggling from
    /// this page's own "remove" button), this re-renders this same page.
    #[serde(default)]
    return_to: String,
}

pub(crate) async fn handler_account_favorites_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<FavoritesForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/_account/favorites").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;

    if form.favorites_action == "set_visibility" {
        let (Some(mut fresh), etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username).await? else {
            return Err(AppError::forbidden(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::account_no_longer_exists(locale)
            )));
        };
        fresh.favorites_public = form.favorites_public == "1";
        let cas = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &fresh, etag.as_deref()).await?;
        let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(locale);
        let flash = match cas {
            Cas::Ok => Flash::Ok("saved."),
            Cas::Conflict => Flash::Err(&cas_conflict_msg),
        };
        let starred = own_starred_with_counts(&ctx, &fresh.starred).await;
        let topnav = login::topnav(&ctx, &headers).await;
        let content = render_favorites_page(&fresh, &csrf_field(&headers), Some(flash), &starred, locale);
        let body = account_settings_page("favorites", content, locale).into_string();
        return Ok(Html(layout(
            "favorites",
            "",
            &body,
            &topnav,
            effective_theme(&ctx, &headers).await,
            effective_density(&ctx, &headers).await,
            locale,
        ))
        .into_response());
    }

    if form.star_action != "toggle_star" {
        return Err(AppError::bad_request(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::unknown_action(locale, &form.star_action)
        )));
    }

    let repo = gitquarry_core::validate::sanitize_path(&form.repo).map_err(AppError::bad_request)?;
    let Some(resolved) = ctx.resolve_repo(&repo).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo_not_found(locale, &repo)
        )));
    };
    // Same check `handler_repo` gates viewing with, and same not-found
    // response on failure: without it, any logged-in user could star (and
    // thus fingerprint the existence of) a private repo they have no grant
    // on — the exists() check above only confirms the repo is in the
    // bucket, not that this user is allowed to know that. Only gates
    // *adding* a star: unstarring an entry already in the user's own list
    // isn't a new disclosure (the name is already on their favorites page)
    // and shouldn't leave them unable to clean up a repo they lost access
    // to after starring it.
    if !me.starred.contains(&resolved.uid) {
        let (repo_config, _etag) = resolved.remote.get_repo_config().await?;
        if !can_view_repo(&ctx, &headers, &repo, repo_config.visibility).await? {
            return Err(AppError::not_found(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::repo_not_found(locale, &repo)
            )));
        }
    }

    // domain::repo::toggle_star re-reads the user's etag right before
    // writing so we don't stomp a concurrent edit: the CAS fails and we
    // report it instead of overwriting.
    let Some((cas, now_starred)) = domain::repo::toggle_star(&ctx, &me.username, &resolved.uid).await? else {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::account_no_longer_exists(locale)
        )));
    };
    if cas == Cas::Ok {
        gitquarry_core::storage::audit::record(
            ctx.backend().as_ref(),
            &ctx.cipher,
            &me.username,
            if now_starred { "repo.star" } else { "repo.unstar" },
            &repo,
            None,
        )
        .await;
    }
    // Re-fetch: `toggle_star` already wrote the new `starred` set, but this
    // handler still needs the full `User` record to render the page below.
    let (Some(me), _etag) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username).await? else {
        return Err(AppError::forbidden(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::account_no_longer_exists(locale)
        )));
    };
    // Authoritative, not the `now_starred` `toggle_star` returned: that
    // value is only guaranteed accurate when `cas == Cas::Ok` (see its own
    // doc comment on the retry loop backing that guarantee) -- on the rare
    // exhausted-retries path it's a placeholder, and trusting it here would
    // tell the browser its own click took even when the write didn't.
    let now_starred = me.starred.contains(&resolved.uid);
    // Async (`data-async`, see `webui::ASYNC_HEADER`'s own doc comment):
    // the button itself is all the caller needs back, not a redirect or
    // the whole favorites page. Checked before the redirect below since a
    // fetch()-driven submit never follows a redirect body as markup.
    //
    // Two elements, not one: the button alone leaves the sidebar's own
    // star count (`community_stats_html`, `repo::summary`) showing the
    // pre-toggle number until a full reload -- `bump_star_count` already
    // wrote the new count by now, so a fresh `get_repo_config` picks it up,
    // and `style/app.js`'s `data-async-extra-target` swaps this second
    // fragment into `#community-stats` independently of the button swap.
    if headers.contains_key(crate::webui::ASYNC_HEADER) {
        let button = crate::webui::star_button_html("/_account/favorites", &repo, &csrf_field(&headers), now_starred);
        let (fresh_config, _etag) = resolved.remote.get_repo_config().await?;
        let stats = crate::webui::repo::community_stats_html(&fresh_config, locale).into_string();
        return Ok(Html(format!("{button}{stats}")).into_response());
    }
    // Only a same-origin relative path is honored — `return_to` is
    // client-submitted, and anything else would be an open redirect. Only
    // set on success: a CAS conflict re-renders the page with the error.
    if cas == Cas::Ok && is_local_redirect(&form.return_to) {
        return Ok(Redirect::to(&form.return_to).into_response());
    }
    let cas_conflict_msg = gitquarry_i18n::shared::cas_conflict(effective_locale(&ctx, &headers).await);
    let flash = match cas {
        Cas::Ok => Flash::Ok("saved."),
        Cas::Conflict => Flash::Err(&cas_conflict_msg),
    };
    let starred = own_starred_with_counts(&ctx, &me.starred).await;
    let topnav = login::topnav(&ctx, &headers).await;
    let content = render_favorites_page(&me, &csrf_field(&headers), Some(flash), &starred, locale);
    let body = account_settings_page("favorites", content, locale).into_string();
    Ok(Html(layout(
        "favorites",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

/// `csrf` is already-rendered trusted HTML (from [`csrf_field`], not user
/// input), so it's spliced in via [`PreEscaped`] rather than escaped like
/// every other interpolated value here — same for [`render_flash`]'s
/// output, which already runs its own message through `esc()`.
pub fn render_favorites_page(
    user: &User,
    csrf: &str,
    message: Option<Flash>,
    starred: &[(String, u32)],
    locale: gitquarry_i18n::Locale,
) -> Markup {
    let action_url = "/_account/favorites";
    html! {
        (PreEscaped(render_flash(message, locale)))

        (settings_section(&crate::auth::i18n::favorites::visibility_heading(locale), Some(&crate::auth::i18n::favorites::visibility_description(locale)), html! {
            form method="post" action=(action_url) {
                (PreEscaped(csrf))
                input type="hidden" name="favorites_action" value="set_visibility";
                label {
                    input type="checkbox" name="favorites_public" value="1" checked[user.favorites_public];
                    (crate::auth::i18n::favorites::public_checkbox_label(locale))
                }
                button type="submit" { (crate::auth::i18n::appearance::save_button(locale)) }
            }
        }))

        (settings_section(&crate::auth::i18n::favorites::starred_heading(locale), Some(&crate::auth::i18n::favorites::starred_intro(locale)), html! {
            @if starred.is_empty() {
                div class="empty-state" {
                    span class="empty-state-icon" { (crate::webui::icons::star()) }
                    div class="empty-state-title" { (crate::auth::i18n::favorites::no_starred_repos_hint(locale)) }
                }
            } @else {
                table class="list" {
                    tr { th { (crate::auth::i18n::favorites::repo_column_header(locale)) } th {} }
                    @for (repo, stars) in starred {
                        tr {
                            td {
                                a href=(format!("/{}", url_encode(repo))) { (repo) }
                                @if *stars > 0 {
                                    " " span class="muted" { (stars) " \u{2605}" }
                                }
                            }
                            td {
                                form method="post" action=(action_url) style="display:inline" {
                                    (PreEscaped(csrf))
                                    // `star_action`, matching `actions.rs`'s
                                    // `star_button_html` -- both forms feed the
                                    // same `FavoritesForm`, whose `star_action`
                                    // field now deserializes from that wire name.
                                    input type="hidden" name="star_action" value="toggle_star";
                                    input type="hidden" name="repo" value=(repo);
                                    button type="submit" { (crate::auth::i18n::favorites::remove_button_label(locale)) }
                                }
                            }
                        }
                    }
                }
            }
        }))
    }
}

/// Read-only counterpart to [`render_favorites_page`] for a viewer who
/// isn't `username` themselves — no unstar buttons, no csrf, just `repos`
/// (already [`visible_starred`]-filtered, so every entry here is safe to
/// show this particular viewer).
pub fn render_public_favorites_page(username: &str, repos: &[(String, u32)], locale: gitquarry_i18n::Locale) -> Markup {
    html! {
        p class="muted" {
            (crate::auth::i18n::favorites::public_favorites_of_prefix(locale)) strong { (username) } "."
        }
        @if repos.is_empty() {
            div class="empty-state" {
                span class="empty-state-icon" { (crate::webui::icons::star()) }
                div class="empty-state-title" { (crate::auth::i18n::favorites::no_favorites_visible_hint(locale)) }
            }
        } @else {
            table class="list" {
                tr { th { (crate::auth::i18n::favorites::repo_column_header(locale)) } }
                @for (repo, stars) in repos {
                    tr {
                        td {
                            a href=(format!("/{}", url_encode(repo))) { (repo) }
                            @if *stars > 0 {
                                " " span class="muted" { (stars) " \u{2605}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
