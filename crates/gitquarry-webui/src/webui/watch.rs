//! `POST /_watch`: toggles the caller's watch on a repo — same
//! [`gitquarry_core::domain::repo::toggle_watch`]/`watcher_count` pair
//! `render_star_button`'s star toggle uses, just its own tiny route
//! instead of living inside `auth::web::favorites` (that module's whole scope is
//! the starred-repos *list* page; watching has no such list yet, so it
//! doesn't need a page of its own — same "smallest thing that does the
//! job" reasoning as [`super::fork`]'s single POST-only route).
//!
//! Watching doesn't currently make anything happen beyond the toggle and
//! the visible `watcher_count` — no notification delivery exists in this
//! codebase.

use super::{ASYNC_HEADER, AppError, State_, csrf_field, effective_locale, require_csrf, require_login, url_encode};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use gitquarry_core::domain;
use gitquarry_core::storage::Cas;
use gitquarry_core::validate;

#[derive(serde::Deserialize)]
pub(crate) struct WatchForm {
    repo: String,
    #[serde(default)]
    csrf: String,
}

pub(crate) async fn handler_watch_post(
    State(ctx): State_,
    headers: HeaderMap,
    Form(form): Form<WatchForm>,
) -> Result<Response, AppError> {
    let me = match require_login(&ctx, &headers, "/").await {
        Ok(u) => u,
        Err(resp) => return Ok(resp),
    };
    require_csrf(&headers, &form.csrf)?;
    let locale = effective_locale(&ctx, &headers).await;

    let repo = validate::sanitize_path(&form.repo).map_err(AppError::bad_request)?;
    let Some(resolved) = ctx.resolve_repo(&repo).await? else {
        return Err(AppError::not_found(anyhow::anyhow!(
            "{}",
            gitquarry_i18n::shared::repo::repo_not_found(locale, &repo)
        )));
    };
    // Same leak-guard `auth::web::favorites`' star toggle applies: only gates
    // starting to watch, not un-watching an entry the caller already has
    // (that's not a new disclosure, and losing a grant shouldn't leave them
    // unable to stop watching something they can no longer see).
    if !me.watched.contains(&resolved.uid) {
        let (config, _etag) = resolved.remote.get_repo_config().await?;
        if !super::repo::can_view_repo(&ctx, &headers, &repo, config.visibility).await? {
            return Err(AppError::not_found(anyhow::anyhow!(
                "{}",
                gitquarry_i18n::shared::repo::repo_not_found(locale, &repo)
            )));
        }
    }

    // A lost CAS race here just means the toggle silently didn't take —
    // acceptable for a display counter with no security consequence,
    // unlike `auth::web::favorites`' star toggle which renders a whole page and
    // can afford to show a conflict flash. Worst case the caller clicks
    // the button again.
    let originally_watched = me.watched.contains(&resolved.uid);
    let now_watched = match domain::repo::toggle_watch(&ctx, &me.username, &resolved.uid).await? {
        Some((Cas::Ok, now_watched)) => {
            gitquarry_core::storage::audit::record(
                ctx.backend().as_ref(),
                &ctx.cipher,
                &me.username,
                if now_watched { "repo.watch" } else { "repo.unwatch" },
                &repo,
                None,
            )
            .await;
            now_watched
        }
        // A lost CAS race (or the user record vanishing mid-request) means
        // the write didn't take -- same "worst case, click again" tolerance
        // the doc comment above already accepts, just also needs to know
        // what to report back for the async fragment below.
        _ => originally_watched,
    };

    // Async (`data-async`, see `ASYNC_HEADER`'s own doc comment): the
    // button itself is all the caller needs back, not a redirect.
    //
    // Two elements, not one -- same reasoning as
    // `auth::web::favorites::handler_favorites_post`'s star toggle: the
    // sidebar's own watcher count needs a fresh `get_repo_config` (after
    // `bump_watcher_count` already wrote it) and its own swap, since it
    // lives outside the button's own subtree.
    if headers.contains_key(ASYNC_HEADER) {
        let button = super::watch_button_html(&repo, &csrf_field(&headers), now_watched);
        let (fresh_config, _etag) = resolved.remote.get_repo_config().await?;
        let stats = super::repo::community_stats_html(&fresh_config, locale).into_string();
        return Ok(Html(format!("{button}{stats}")).into_response());
    }

    Ok(Redirect::to(&format!("/{}", url_encode(&repo))).into_response())
}
