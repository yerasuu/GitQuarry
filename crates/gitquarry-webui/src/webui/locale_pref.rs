//! Which locale a request's user-facing text should render in.

use super::cookies::get_cookie;
use axum::http::HeaderMap;

pub(crate) const LOCALE_COOKIE: &str = "locale";

/// `None` means "no explicit session override yet" — same convention as
/// [`super::theme_pref::cookie_theme`]. `pub(super)`, not private:
/// [`super::csrf::require_csrf`] also falls back to this for its own error
/// message's locale, without an `AppContext` in hand to call
/// [`effective_locale`] properly.
pub fn cookie_locale(headers: &HeaderMap) -> Option<gitquarry_i18n::Locale> {
    get_cookie(headers, LOCALE_COOKIE).and_then(|s| gitquarry_i18n::Locale::parse(&s))
}

/// The locale a request's user-facing text should render in: this
/// session's override if the viewer already picked one, otherwise the
/// logged-in user's own account default, otherwise
/// [`gitquarry_i18n::Locale::DEFAULT`]. Same "explicit choice beats account
/// default beats built-in default" precedence as
/// [`super::theme_pref::effective_theme`]/
/// [`super::text_size::effective_text_size`] — there's no `/_prefs/locale`
/// toggle UI yet (see `src/i18n/mod.rs`'s own doc comment on what this
/// first slice does and doesn't include), so today this only ever resolves
/// via the account default or the built-in fallback; the cookie check is
/// here so later commits can add the toggle without touching this function
/// again.
pub(crate) async fn effective_locale(
    ctx: &gitquarry_core::app::AppContext,
    headers: &HeaderMap,
) -> gitquarry_i18n::Locale {
    if let Some(locale) = cookie_locale(headers) {
        return locale;
    }
    match crate::auth::web::login::current_user(ctx, headers)
        .await
        .and_then(|u| gitquarry_i18n::Locale::parse(&u.locale_default))
    {
        Some(locale) => locale,
        None => gitquarry_i18n::Locale::DEFAULT,
    }
}

tokio::task_local! {
    /// This request's own [`effective_locale`], scoped by
    /// `webui::request_cache_middleware` alongside
    /// [`gitquarry_core::app::REQUEST_BACKEND`]/
    /// [`crate::auth::web::login::CURRENT_USER_MEMO`]. Exists for
    /// [`super::error::AppError`]'s `IntoResponse` impl, which -- being a
    /// sync trait method with no `HeaderMap`/`AppContext` of its own --
    /// can't call `effective_locale` directly; every other handler still
    /// calls `effective_locale` itself and has no reason to touch this.
    pub(crate) static CURRENT_LOCALE: gitquarry_i18n::Locale;
}
