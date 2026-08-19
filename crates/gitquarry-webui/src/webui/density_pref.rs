//! Dense/airy density preference — a second, independent display-preference
//! axis alongside [`super::theme_pref`] (accessibility: some people want
//! tighter layouts, some want more breathing room, regardless of color
//! theme). Same cookie-then-account-default mechanism as theme, reusing
//! [`super::theme_pref::referer_path`] rather than duplicating its
//! open-redirect hardening.

use super::cookies::{get_cookie, set_cookie};
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};

pub(crate) const DENSITY_COOKIE: &str = "density";

/// `None` means "no explicit session override yet" — same convention as
/// [`super::theme_pref::cookie_theme`].
pub fn cookie_density(headers: &HeaderMap) -> Option<&'static str> {
    match get_cookie(headers, DENSITY_COOKIE).as_deref() {
        Some("dense") => Some("dense"),
        Some("airy") => Some("airy"),
        _ => None,
    }
}

/// `data-density` value for `layout`'s `<html>`: `"dense"`/`"airy"`/`""` —
/// this session's override if the viewer already clicked the toggle,
/// otherwise the logged-in user's own account default, otherwise empty.
/// Unlike theme, empty has no OS-level signal to fall back to — it just
/// means the `:root` default, which is dense (see [`super::page_chrome::STYLE`]).
/// Same "explicit choice beats account default beats built-in default"
/// precedence as [`super::theme_pref::effective_theme`].
pub(crate) async fn effective_density(ctx: &gitquarry_core::app::AppContext, headers: &HeaderMap) -> &'static str {
    if let Some(density) = cookie_density(headers) {
        return density;
    }
    match crate::auth::web::login::current_user(ctx, headers)
        .await
        .map(|u| u.density_default)
    {
        Some(ref s) if s == "dense" => "dense",
        Some(ref s) if s == "airy" => "airy",
        _ => "",
    }
}

#[derive(serde::Deserialize)]
pub struct DensityForm {
    /// `"dense"`/`"airy"` — always an explicit pick, same shape as
    /// [`super::theme_pref::ThemeForm::theme`].
    pub density: String,
}

/// Sets an explicit `density` override to whichever of the two buttons was
/// clicked and redirects back to the page it was clicked from — same
/// reasoning and same no-login-needed, no-CSRF (stateless display
/// preference, not a credential) shape as [`super::theme_pref::handler_toggle_theme`],
/// including [`super::theme_pref::sync_account_pref`] updating the account
/// default when logged in.
pub async fn handler_toggle_density(
    State(ctx): super::State_,
    headers: HeaderMap,
    Form(form): Form<DensityForm>,
) -> Response {
    let new_value = if form.density == "airy" { "airy" } else { "dense" };
    super::theme_pref::sync_account_pref(&ctx, &headers, new_value, |u| &mut u.density_default).await;
    let (header, value) = set_cookie(&headers, DENSITY_COOKIE, new_value, 365 * 24 * 60 * 60);
    let mut resp = if headers.contains_key(super::ASYNC_HEADER) {
        axum::response::Html(render_density_toggle(new_value).into_string()).into_response()
    } else {
        Redirect::to(&super::theme_pref::referer_path(&headers)).into_response()
    };
    resp.headers_mut()
        .insert(header, value.parse().expect("density cookie always valid"));
    resp
}

/// The two-button dense/airy toggle rendered in `layout`'s topbar, next to
/// the theme toggle. Unlike theme's empty-state (no highlight — a real
/// "follow OS" state exists), density's `""` visually reads as dense: that's
/// the actual rendered state (see [`effective_density`]'s doc comment), so
/// the dense button is active whenever `density != "airy"`, not only when
/// it's the literal string `"dense"`.
pub fn render_density_toggle(density: &str) -> maud::Markup {
    maud::html! {
        // See `theme_pref::render_theme_toggle`'s matching comment on
        // `data-html-attr` — same gap, same fix, same reason `<html>`
        // itself never got the pick until a reload.
        form method="post" action="/_prefs/density" style="display:inline" data-async data-html-attr="data-density" {
            button type="submit" name="density" value="dense"
                class={"density-btn" @if density != "airy" { " active" }} title="dense layout" aria-label="dense layout" aria-pressed=(if density != "airy" { "true" } else { "false" }) { "▤" }
            button type="submit" name="density" value="airy"
                class={"density-btn" @if density == "airy" { " active" }} title="airy layout" aria-label="airy layout" aria-pressed=(if density == "airy" { "true" } else { "false" }) { "▥" }
        }
    }
}
