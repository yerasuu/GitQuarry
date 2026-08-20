//! `/_changelog`: the running build's own changelog.
//!
//! Embedded with `include_str!`, not read from disk or fetched from a repo:
//! the file that ships inside a binary is the one that describes *that*
//! binary, so the page can never drift from what the instance is actually
//! running — which is the whole reason a reader opens it. It also means the
//! page works on a deploy that doesn't host gitquarry's own source.
//!
//! Public, with no login gate: it is release documentation, the same text
//! the repository publishes to anyone. Nothing here is instance-specific.

use crate::webui::{AppError, State_, effective_density, effective_locale, effective_theme, layout};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};

/// The notes for the version this binary *is*: the path is built from
/// `CARGO_PKG_VERSION`, so a release cut without its notes file fails the
/// build instead of quietly shipping a page describing an older one.
const RELEASE_NOTES: &str = include_str!(concat!("../../../../tag-changelog/v", env!("CARGO_PKG_VERSION"), ".md"));

pub(crate) async fn handler_changelog_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let topnav = crate::auth::web::login::topnav(&ctx, &headers).await;
    let body = maud::html! {
        h2 { (gitquarry_i18n::web::chrome::changelog_heading(locale)) }
        p class="muted" {
            (gitquarry_i18n::web::chrome::changelog_running_version(locale, env!("CARGO_PKG_VERSION")))
        }
        // `readme`, not a class of its own: every markdown style in the
        // sheet is scoped under it, and this is the same kind of content.
        div class="readme" { (maud::PreEscaped(super::markdown::render(RELEASE_NOTES))) }
    }
    .into_string();
    Ok(Html(layout(
        "changelog",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}
