//! `/_audit`: admin-only read-only feed of [`gitquarry_core::storage::audit`]'s recent
//! entries — who did what to what, and when. No filters, no pagination:
//! just the last [`gitquarry_core::storage::audit::LIST_LIMIT`], same "recent activity"
//! scope as everything else this bucket-side trail is for.

use super::{State_, admin_header, effective_density, effective_locale, effective_theme, layout};
use crate::auth::web::login;
use crate::settings::web::require_admin;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use gitquarry_core::storage::audit::{self, AuditEntry};
use gitquarry_i18n::Locale;
use gitquarry_i18n::web::audit as t;
use maud::{Markup, html};

pub(crate) async fn handler_audit_get(State(ctx): State_, headers: HeaderMap) -> Result<Response, super::AppError> {
    if let Some(resp) = require_admin(&ctx, &headers, "/_audit").await {
        return Ok(resp);
    }
    let entries = audit::list_recent(&ctx.backend(), &ctx.cipher).await?;
    let topnav = login::topnav(&ctx, &headers).await;
    let locale = effective_locale(&ctx, &headers).await;
    let body = render_audit_page(&entries, locale).into_string();
    Ok(axum::response::Html(layout(
        "audit log",
        &admin_header("audit", locale),
        &body,
        &topnav,
        effective_theme(&ctx, &headers).await,
        effective_density(&ctx, &headers).await,
        locale,
    ))
    .into_response())
}

pub fn render_audit_page(entries: &[AuditEntry], locale: Locale) -> Markup {
    html! {
        p class="muted" {
            (t::audit_intro_prefix(locale, audit::LIST_LIMIT))
            code { ".conf/audit/" }
            (t::audit_intro_suffix(locale))
        }
        div class="table-scroll" {
            table class="list" {
                tr {
                    th { (t::when_column(locale)) }
                    th { (t::who_column(locale)) }
                    th { (t::action_column(locale)) }
                    th { (t::on_column(locale)) }
                    th { (t::detail_column(locale)) }
                }
                @if entries.is_empty() {
                    tr { td colspan="5" {
                        div class="empty-state" {
                            span class="empty-state-icon" { (crate::webui::icons::lock()) }
                            div class="empty-state-title" { (t::no_entries_hint(locale)) }
                        }
                    } }
                }
                @for e in entries {
                    tr {
                        td class="muted" { (e.at) }
                        td { (e.actor) }
                        td { code { (e.action) } }
                        td { (e.target) }
                        td class="muted" { (e.detail.as_deref().unwrap_or("")) }
                    }
                }
            }
        }
    }
}
