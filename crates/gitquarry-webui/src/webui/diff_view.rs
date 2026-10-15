//! A diff's rendering: the traditional single-column `+`/`-` stream, or a
//! two-column old-vs-new layout.

use super::cookies::{get_cookie, set_cookie};
use super::escaping::is_local_redirect;
use axum::extract::Form;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};

/// Session-only preference (cookie, no account default) — unlike
/// theme/text-size this isn't tied to a `User` field, there was no request
/// to make it one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DiffView {
    Unified,
    Split,
}

impl DiffView {
    fn as_cookie(self) -> &'static str {
        match self {
            DiffView::Unified => "unified",
            DiffView::Split => "split",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "unified" => Some(DiffView::Unified),
            "split" => Some(DiffView::Split),
            _ => None,
        }
    }
}

pub(crate) const DIFF_VIEW_COOKIE: &str = "diff-view";

/// This session's pick, or `Unified` (today's only behavior, so existing
/// viewers see no change) if nothing's been picked yet.
pub(crate) fn effective_diff_view(headers: &HeaderMap) -> DiffView {
    get_cookie(headers, DIFF_VIEW_COOKIE)
        .and_then(|s| DiffView::parse(&s))
        .unwrap_or(DiffView::Unified)
}

#[derive(serde::Deserialize)]
pub(crate) struct DiffViewForm {
    /// `"unified"`/`"split"` — always an explicit pick, same shape as
    /// [`super::text_size::TextSizeForm::size`].
    view: String,
    #[serde(default)]
    return_to: String,
}

/// Same reasoning as [`super::text_size::handler_toggle_text_size`]: a
/// rendering preference anyone gets to pick for their own session, logged
/// in or not, no CSRF needed (worst case a forged request flips how a diff
/// is drawn, not anything that touches data or permissions).
pub(crate) async fn handler_toggle_diff_view(headers: HeaderMap, Form(form): Form<DiffViewForm>) -> Response {
    let new_value = DiffView::parse(&form.view).unwrap_or(DiffView::Unified).as_cookie();
    let (header, value) = set_cookie(&headers, DIFF_VIEW_COOKIE, new_value, 365 * 24 * 60 * 60);
    let target = if is_local_redirect(&form.return_to) {
        form.return_to.as_str()
    } else {
        "/"
    };
    let mut resp = Redirect::to(target).into_response();
    resp.headers_mut()
        .insert(header, value.parse().expect("diff-view cookie always valid"));
    resp
}

/// Two-button unified/split toggle, rendered directly above a diff by
/// whichever view has one (`commit`, `compare`) — `return_to` is that same
/// page's own URL, built by the caller (there's no single "current page" the
/// way `render_text_size_buttons` has for the summary tab).
pub(crate) fn render_diff_view_buttons(return_to: &str, current: DiffView, locale: gitquarry_i18n::Locale) -> String {
    use gitquarry_i18n::web::repo::components as t;
    let buttons = [
        (DiffView::Unified, t::diff_view_unified_button(locale)),
        (DiffView::Split, t::diff_view_split_button(locale)),
    ];
    maud::html! {
        @for (view, label) in buttons {
            form method="post" action="/_prefs/diff_view" class="inline-form" {
                input type="hidden" name="view" value=(view.as_cookie());
                input type="hidden" name="return_to" value=(return_to);
                button type="submit" class={"size-btn" @if view == current { " active" }} { (label) }
            }
        }
    }
    .into_string()
}
