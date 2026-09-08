//! Light/dark theme preference.

use super::cookies::{get_cookie, set_cookie};
use super::escaping::is_local_redirect;
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use gitquarry_core::auth::access;

pub(crate) const THEME_COOKIE: &str = "theme";

/// `None` means "no explicit session override yet" — same convention as
/// [`super::text_size::cookie_text_size`], so [`effective_theme`] knows to
/// keep falling back instead of treating an absent/garbage cookie as a
/// choice.
pub fn cookie_theme(headers: &HeaderMap) -> Option<&'static str> {
    match get_cookie(headers, THEME_COOKIE).as_deref() {
        Some("dark") => Some("dark"),
        Some("light") => Some("light"),
        _ => None,
    }
}

/// `data-theme` value for `layout`'s `<html>`: `"dark"`/`"light"`/`""` — this
/// session's override if the viewer already clicked the toggle, otherwise
/// the logged-in user's own account default, otherwise empty (nothing
/// overrides `prefers-color-scheme`, see `STYLE`'s `:root[data-theme]`
/// rules). Same "explicit choice beats account default beats global
/// default" precedence as [`super::text_size::effective_text_size`].
pub(crate) async fn effective_theme(ctx: &gitquarry_core::app::AppContext, headers: &HeaderMap) -> &'static str {
    if let Some(theme) = cookie_theme(headers) {
        return theme;
    }
    match crate::auth::web::login::current_user(ctx, headers)
        .await
        .map(|u| u.theme_default)
    {
        Some(ref s) if s == "dark" => "dark",
        Some(ref s) if s == "light" => "light",
        _ => "",
    }
}

#[derive(serde::Deserialize)]
pub struct ThemeForm {
    /// `"dark"`/`"light"` — always an explicit pick, same shape as
    /// [`super::text_size::TextSizeForm::size`].
    pub theme: String,
}

/// Same-origin path+query out of the request's own `Referer` header, so the
/// theme toggle can bounce back to whatever page it was clicked from without
/// a `return_to` field threaded through every page (unlike text-size and
/// star, the theme toggle lives in the shared topbar, not one page's form).
/// Trusting `Referer` blindly would be exactly the open-redirect this
/// project has already hardened `return_to` against twice, so this only
/// accepts a `Referer` whose scheme+host matches this request's own `Host`
/// header; anything else — missing, unparseable, cross-host — falls back to
/// `/`.
pub fn referer_path(headers: &HeaderMap) -> String {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let referer = headers
        .get(axum::http::header::REFERER)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !host.is_empty() {
        for scheme in ["http://", "https://"] {
            if let Some(after_host) = referer.strip_prefix(scheme).and_then(|rest| rest.strip_prefix(host)) {
                if after_host.is_empty() {
                    return "/".to_string();
                }
                if is_local_redirect(after_host) {
                    return after_host.to_string();
                }
            }
        }
    }
    "/".to_string()
}

/// Sets an explicit `theme` override to whichever of the two buttons was
/// clicked and redirects back to the page it was clicked from. No login
/// needed — an anonymous click still gets the cookie, same reasoning as
/// [`super::text_size::handler_toggle_text_size`]: a light/dark pick is a
/// display preference, not a credential. Logged in, it also becomes this
/// account's own default ([`sync_account_pref`]) — the same pick then
/// follows to every other device, not just this browser's cookie.
/// Async (`data-async`, see `ASYNC_HEADER`'s own doc comment): the browser
/// applies a fetch() response's `Set-Cookie` exactly like a normal one, so
/// the cookie still gets set here — the only difference is the body is
/// [`render_theme_toggle`]'s own fragment instead of a redirect, since a
/// `fetch()` call never follows/renders a redirect as markup.
pub async fn handler_toggle_theme(
    State(ctx): super::State_,
    headers: HeaderMap,
    Form(form): Form<ThemeForm>,
) -> Response {
    let new_value = if form.theme == "dark" { "dark" } else { "light" };
    sync_account_pref(&ctx, &headers, new_value, |u| &mut u.theme_default).await;
    let (header, value) = set_cookie(&headers, THEME_COOKIE, new_value, 365 * 24 * 60 * 60);
    let mut resp = if headers.contains_key(super::ASYNC_HEADER) {
        axum::response::Html(render_theme_toggle(new_value).into_string()).into_response()
    } else {
        Redirect::to(&referer_path(&headers)).into_response()
    };
    resp.headers_mut()
        .insert(header, value.parse().expect("theme cookie always valid"));
    resp
}

/// Best-effort: if `headers` carries a logged-in session, updates that
/// account's own default (whichever field `field` projects to — see
/// [`super::density_pref::handler_toggle_density`]/
/// [`super::text_size::handler_toggle_text_size`] for the other two
/// callers) to `new_value`, so a topbar quick-toggle click also becomes
/// this account's default everywhere, not just a cookie on this one
/// browser. One write attempt, no CAS retry loop — a lost race just means
/// this particular click didn't update the account default, no worse than
/// not having clicked, and the next toggle tries again. Skips the write
/// entirely when the account default already matches, so repeatedly
/// clicking the same already-active button doesn't burn a write every time.
pub(crate) async fn sync_account_pref(
    ctx: &gitquarry_core::app::AppContext,
    headers: &HeaderMap,
    new_value: &str,
    field: impl Fn(&mut gitquarry_core::auth::entities::User) -> &mut String,
) {
    let Some(me) = crate::auth::web::login::current_user(ctx, headers).await else {
        return;
    };
    let Ok((Some(mut user), etag)) = access::get_user(ctx.backend().as_ref(), &ctx.cipher, &me.username).await else {
        return;
    };
    if field(&mut user).as_str() == new_value {
        return;
    }
    *field(&mut user) = new_value.to_string();
    let _ = access::put_user(ctx.backend().as_ref(), &ctx.cipher, &user, etag.as_deref()).await;
}

/// The two-button light/dark toggle rendered in `layout`'s topbar — bold
/// highlight on whichever matches `theme` (same active-highlight convention
/// as `render_text_size_buttons`), neither highlighted when `theme` is
/// empty (no explicit pick yet, still following `prefers-color-scheme`).
/// One `data-async` form with two named submit buttons (`name="theme"`,
/// each its own `value`), not two separate forms: the async path replaces
/// whichever form was submitted with this same fragment, so both buttons'
/// active state has to live in that one replaced element, not a sibling
/// left untouched. `pub(super)`, not private: [`super::page_chrome::layout`]
/// renders this directly in the topbar.
pub fn render_theme_toggle(theme: &str) -> maud::Markup {
    maud::html! {
        // `data-html-attr="data-theme"`: `style/app.js`'s submit handler
        // applies the clicked button's own value to `<html data-theme>`
        // the instant it's clicked, not just on the next full page load —
        // the async response here only ever re-rendered this button pair,
        // never touched `<html>` itself, so the toggle *looked* like it did
        // nothing until a reload happened to pick the new cookie back up.
        form method="post" action="/_prefs/theme" class="inline-form" data-async data-html-attr="data-theme" {
            button type="submit" name="theme" value="light"
                class={"theme-btn" @if theme == "light" { " active" }} title="light mode" aria-label="light mode" aria-pressed=(if theme == "light" { "true" } else { "false" }) { "☀" }
            button type="submit" name="theme" value="dark"
                class={"theme-btn" @if theme == "dark" { " active" }} title="dark mode" aria-label="dark mode" aria-pressed=(if theme == "dark" { "true" } else { "false" }) { "☾" }
        }
    }
}
