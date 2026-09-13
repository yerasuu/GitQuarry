//! `/_license`: the licence this binary is actually distributed under.
//!
//! The AGPL obliges an operator to offer the terms to the people using the
//! service over a network, not merely to whoever downloads the source — and
//! a link to gnu.org doesn't discharge that: it points at a copy the
//! project doesn't control, which could differ from, outlive or drift from
//! the one shipped here. So the footer links to the file itself, embedded
//! with `include_str!` for the same reason [`super::changelog`] is: what a
//! reader gets is the text compiled into the binary they are talking to.
//!
//! Served as preformatted text rather than markdown. The GPL family's plain
//! text is laid out by hand — hanging indents, hard-wrapped columns,
//! numbered clauses — and a markdown renderer collapses exactly that into
//! paragraphs, silently reflowing a legal document.

use crate::webui::{AppError, State_, effective_density, effective_locale, effective_theme, layout};
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};

const LICENSE: &str = include_str!("../../../../LICENSE");

pub(crate) async fn handler_license_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, AppError> {
    let locale = effective_locale(&ctx, &headers).await;
    let topnav = crate::auth::web::login::topnav(&ctx, &headers).await;
    let body = maud::html! {
        h2 { (gitquarry_i18n::web::chrome::license_heading(locale)) }
        pre class="license-text" { (LICENSE) }
    }
    .into_string();
    Ok(Html(layout(
        "license",
        "",
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}
