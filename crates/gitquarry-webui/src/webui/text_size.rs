//! The about tab's own text-size preference, distinct from and unaffected
//! by the tabs bar above it, which never changes size.

use super::cookies::{get_cookie, set_cookie};
use super::escaping::is_local_redirect;
use axum::extract::{Form, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};

/// Three tiers, not a plain large/small flag: `Sm` is the original
/// pre-ergonomics-pass size, `Md` is what `body`'s own (larger) font-size
/// already gives everything else on the page, `Lg` is bigger still.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextSize {
    Sm,
    Md,
    Lg,
}

impl TextSize {
    fn as_cookie(self) -> &'static str {
        match self {
            TextSize::Sm => "sm",
            TextSize::Md => "md",
            TextSize::Lg => "lg",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "sm" => Some(TextSize::Sm),
            "md" => Some(TextSize::Md),
            "lg" => Some(TextSize::Lg),
            _ => None,
        }
    }

    /// Class the about view wraps its body in — `Md` needs none, it's
    /// just `body`'s own already-cascading size.
    pub(crate) fn div_class(self) -> &'static str {
        match self {
            TextSize::Sm => "text-sm",
            TextSize::Md => "",
            TextSize::Lg => "text-lg",
        }
    }
}

pub(crate) const TEXT_SIZE_COOKIE: &str = "text-size";

/// `None` means "no explicit session override yet" — deliberately not the
/// same as `Sm`, so [`effective_text_size`] knows to keep falling back
/// instead of treating an absent cookie as a choice.
fn cookie_text_size(headers: &HeaderMap) -> Option<TextSize> {
    TextSize::parse(&get_cookie(headers, TEXT_SIZE_COOKIE).unwrap_or_default())
}

/// This session's override if the viewer already clicked the toggle,
/// otherwise the logged-in user's own account default, otherwise `Sm` (the
/// original size) — explicit choice beats account default beats global
/// default.
pub(crate) async fn effective_text_size(ctx: &gitquarry_core::app::AppContext, headers: &HeaderMap) -> TextSize {
    if let Some(size) = cookie_text_size(headers) {
        return size;
    }
    crate::auth::web::login::current_user(ctx, headers)
        .await
        .and_then(|u| TextSize::parse(&u.text_size_default))
        .unwrap_or(TextSize::Sm)
}

#[derive(serde::Deserialize)]
pub(crate) struct TextSizeForm {
    /// `"sm"`/`"md"`/`"lg"` — one of the three buttons, always an explicit
    /// pick, never a "toggle from whatever's current" cycle.
    size: String,
    /// Where to redirect back to — always the page the button was on, same
    /// `return_to` convention the star-toggle button already uses.
    #[serde(default)]
    return_to: String,
}

/// Sets an explicit `text-size` override to whichever of the three buttons
/// was clicked and redirects back to `return_to`. No login needed: an
/// anonymous click still gets the session cookie, same as
/// [`super::theme_pref::handler_toggle_theme`] — logged in, it also becomes
/// this account's own default ([`super::theme_pref::sync_account_pref`]),
/// the same self-service field `crate::auth::web::appearance`'s
/// `set_text_size_pref` action sets from the account-settings page.
///
/// Async (`data-async`, see `ASYNC_HEADER`'s own doc comment): same shape
/// [`super::theme_pref::handler_toggle_theme`] uses — the fetch()-driven
/// submit gets [`render_text_size_buttons`]'s fragment back (re-rendered
/// with the new active state) instead of a redirect it could never follow
/// as markup, while the `Set-Cookie` above still lands either way.
pub(crate) async fn handler_toggle_text_size(
    State(ctx): super::State_,
    headers: HeaderMap,
    Form(form): Form<TextSizeForm>,
) -> Response {
    let new_size = TextSize::parse(&form.size).unwrap_or(TextSize::Sm);
    super::theme_pref::sync_account_pref(&ctx, &headers, new_size.as_cookie(), |u| &mut u.text_size_default).await;
    let (header, value) = set_cookie(&headers, TEXT_SIZE_COOKIE, new_size.as_cookie(), 365 * 24 * 60 * 60);
    let target = if is_local_redirect(&form.return_to) {
        form.return_to.as_str()
    } else {
        "/"
    };
    let mut resp = if headers.contains_key(super::ASYNC_HEADER) {
        axum::response::Html(render_text_size_buttons(target, new_size)).into_response()
    } else {
        Redirect::to(target).into_response()
    };
    resp.headers_mut()
        .insert(header, value.parse().expect("text-size cookie always valid"));
    resp
}

/// Three explicit picks (not a hidden-field toggle like `render_star_button`,
/// not a cycling single button either) directly on `/_prefs/text_size`.
/// Rendered in `header_bar`'s tabs-bar right side, only on the about tab
/// (see its callers in `repo::handler_repo`): the about view is the only
/// one that reads the same effective text-size preference to size its own
/// body, so these would do nothing visible anywhere else. `current` gets
/// the same `active` styling the tabs themselves use, so it's clear at a
/// glance which of the three is in effect.
/// `id="text-size-buttons"` wraps all three: `data-async-target` on each
/// form points back at this whole group (not just the form that was
/// submitted), so picking `lg` also un-highlights `sm`/`md` in the same
/// swap — same "more than the form itself has to refresh" shape
/// `merges/mod.rs`'s `#mr-actions` already uses. `data-class-target`/
/// `data-class-value`: same "paint it before the round trip" idea as
/// [`super::theme_pref::render_theme_toggle`]'s `data-html-attr`, just
/// aimed at `#about-body`'s own class instead of `<html>`'s attribute —
/// this preference scopes to the about tab's body, not the whole page,
/// so there's no single global attribute for it to piggyback on.
pub(crate) fn render_text_size_buttons(return_to: &str, current: TextSize) -> String {
    maud::html! {
        span id="text-size-buttons" {
            @for (size, glyph, title) in [(TextSize::Sm, "A\u{2212}", "small"), (TextSize::Md, "A", "medium"), (TextSize::Lg, "A+", "large")] {
                form method="post" action="/_prefs/text_size" class="inline-form" data-async
                    data-async-target="#text-size-buttons"
                    data-class-target="#about-body" data-class-value=(size.div_class()) {
                    input type="hidden" name="size" value=(size.as_cookie());
                    input type="hidden" name="return_to" value=(return_to);
                    button type="submit" class={"size-btn" @if size == current { " active" }} title=(title) aria-label=(title) { (glyph) }
                }
            }
        }
    }
    .into_string()
}
