//! Star/watch toggle buttons, rendered on every repo page.

use super::csrf::csrf_field;
use super::escaping::url_encode;
use axum::http::HeaderMap;

/// Pure markup for the star/unstar form, `starred` already resolved by the
/// caller — factored out of [`render_star_button`] so
/// `auth::web::favorites::handler_favorites_post` can re-render just this
/// fragment after an async (`data-async`) toggle, using the fresh state it
/// already has in hand, instead of this function re-deriving it from
/// scratch (which would risk reading a request-scoped cached `User` from
/// *before* the toggle it just performed — see `app::REQUEST_BACKEND`'s own
/// doc comment on why that cache exists and how it's scoped).
pub(crate) fn star_button_html(
    favorites_url: &str,
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    csrf_html: &str,
    starred: bool,
) -> String {
    let (label, toggle_label, delta) = if starred {
        ("unstar", "star", "-1")
    } else {
        ("star", "unstar", "1")
    };
    // ids/selectors carry `repo_uid`, not just `star-count-value`: this
    // button and `repo::summary::community_stats_html`'s row are the only
    // two places on a page today, but a fixed id would silently bump the
    // wrong repo's count the moment either renders more than once per page
    // (a multi-repo list, say) — the uid keeps every instance unambiguous
    // regardless of where it ends up rendered.
    let count_target = format!("#star-count-value-{repo_uid}");
    let stats_target = format!("#community-stats-{repo_uid}");
    maud::html! {
        // `data-toggle-label`/`data-optimistic-target`/`data-optimistic-delta`:
        // `style/app.js`'s submit handler flips the button's own label and
        // bumps the count by `delta` *before* the fetch, not after -- the
        // round trip (~½s over a real network) would otherwise be visible
        // lag on every click. The real response still lands moments later
        // and fully replaces this form/the count (`data-async`/
        // `data-async-extra-target`), so a wrong optimistic guess (a lost
        // CAS race, a second tab) self-corrects within the same round trip
        // instead of drifting.
        form method="post" action=(favorites_url) style="display:inline" data-async
            data-async-extra-target=(stats_target)
            data-toggle-label=(toggle_label) data-optimistic-target=(count_target) data-optimistic-delta=(delta) {
            (maud::PreEscaped(csrf_html))
            // NOT `name="action"`: `HTMLFormElement` is `[OverrideBuiltins]`,
            // so a control literally named `action` shadows the form's own
            // `.action` IDL property -- `form.action` in `style/app.js`'s
            // `fetchFragment(form.action, ...)` then returns this `<input>`
            // element instead of the URL string, and `fetch()` silently
            // stringifies it to `"[object HTMLInputElement]"`, 404-ing.
            input type="hidden" name="star_action" value="toggle_star";
            input type="hidden" name="repo" value=(repo_name);
            input type="hidden" name="return_to" value=(format!("/{}", url_encode(repo_name)));
            button type="submit" class="action-btn" {
                (crate::webui::icons::star()) " " span class="btn-label" { (label) }
            }
        }
    }
    .into_string()
}

/// A star/unstar button, only for a logged-in viewer — starring is purely
/// personal (see [`gitquarry_core::auth::entities::User::starred`]), so there's
/// nothing to show an anonymous one. Posts to `/_favorites` (that's where
/// `starred` actually lives) with `return_to` set back to this same page,
/// so toggling doesn't bounce the viewer off the repo they're looking at.
/// Rendered in `header_bar`'s tabs-bar right side on every tab, not just
/// summary — starring a repo isn't a summary-specific action the way the
/// text-size toggle is. `data-async` (see [`star_button_html`]): the button
/// itself already carries everything `style/app.js`'s delegated listener
/// needs, no extra wiring per page.
pub(crate) async fn render_star_button(
    ctx: &gitquarry_core::app::AppContext,
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    headers: &HeaderMap,
) -> String {
    let Some(me) = crate::auth::web::login::current_user(ctx, headers).await else {
        return String::new();
    };
    let starred = me.starred.contains(repo_uid);
    star_button_html(
        "/_account/favorites",
        repo_name,
        repo_uid,
        &csrf_field(headers),
        starred,
    )
}

/// Pure markup for the watch/unwatch form — same reasoning as
/// [`star_button_html`], reused by
/// `webui::watch::handler_watch_post`'s async response.
pub(crate) fn watch_button_html(
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    csrf_html: &str,
    watched: bool,
) -> String {
    let (label, toggle_label, delta) = if watched {
        ("unwatch", "watch", "-1")
    } else {
        ("watch", "unwatch", "1")
    };
    // Same `repo_uid`-scoped ids as `star_button_html` -- see its own doc
    // comment on why a fixed id isn't safe to reuse across repos.
    let count_target = format!("#watcher-count-value-{repo_uid}");
    let stats_target = format!("#community-stats-{repo_uid}");
    maud::html! {
        // Optimistic label/count flip -- see `star_button_html`'s own doc
        // comment on `data-toggle-label`/`data-optimistic-target`/
        // `data-optimistic-delta`.
        form method="post" action="/_watch" style="display:inline" data-async
            data-async-extra-target=(stats_target)
            data-toggle-label=(toggle_label) data-optimistic-target=(count_target) data-optimistic-delta=(delta) {
            (maud::PreEscaped(csrf_html))
            input type="hidden" name="repo" value=(repo_name);
            button type="submit" class="action-btn" {
                (crate::webui::icons::eye()) " " span class="btn-label" { (label) }
            }
        }
    }
    .into_string()
}

/// A watch/unwatch button, exact mirror of [`render_star_button`] — same
/// logged-in-only gate, same `return_to`-back-to-this-page shape, posts to
/// [`crate::webui::watch::handler_watch_post`] (its own tiny route, not
/// `/_favorites` — see that module's doc for why watching doesn't share
/// the starred-list page). Watching doesn't currently do anything beyond
/// this toggle and the visible `watcher_count` — no notification system
/// exists in this codebase.
pub(crate) async fn render_watch_button(
    ctx: &gitquarry_core::app::AppContext,
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    headers: &HeaderMap,
) -> String {
    let Some(me) = crate::auth::web::login::current_user(ctx, headers).await else {
        return String::new();
    };
    let watched = me.watched.contains(repo_uid);
    watch_button_html(repo_name, repo_uid, &csrf_field(headers), watched)
}

/// [`render_star_button`] + [`render_watch_button`] concatenated — every
/// `header_bar`/settings-page call site wants both together, so this is the
/// one place that does it instead of each of the 5 call sites separately.
/// No wrapper element: `.action-btn` (shared with the theme/density/text-
/// size toggles, see its own CSS doc comment) already gives each button its
/// own small margin, the same "several small buttons in a row" look those
/// already have — a `.btn-group`'s connected-pill joining is a different,
/// deliberately not chosen here now that these carry that same class
/// instead of styler's own `.btn`. Empty for an anonymous viewer, same as
/// either button alone.
pub(crate) async fn render_repo_action_buttons(
    ctx: &gitquarry_core::app::AppContext,
    repo_name: &str,
    repo_uid: &gitquarry_core::storage::uid::Uid,
    headers: &HeaderMap,
) -> String {
    if crate::auth::web::login::current_user(ctx, headers).await.is_none() {
        return String::new();
    }
    format!(
        "{}{}",
        render_star_button(ctx, repo_name, repo_uid, headers).await,
        render_watch_button(ctx, repo_name, repo_uid, headers).await,
    )
}
